#include "program_builder.h"

#include <algorithm>
#include <functional>
#include <queue>
#include <set>

#include "clang/AST/Mangle.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/SourceManager.h"
#include "llvm/Support/ErrorHandling.h"

#include "aie_check.h"
#include "build_program.h"
#include "classify.h"
#include "codegen/aie.h"
#include "codegen/conventions.h"
#include "codegen/ignore.h"
#include "codegen/rewrite.h"
#include "codegen/schema_fields.h"
#include "codegen/tree_writer.h"
#include "codegen/xilinx.h"
#include "diag.h"
#include "discover.h"
#include "invoke_parser.h"
#include "names.h"
#include "ports.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

// "file:line:col" as the user wrote it. Locations outlive their TU only in
// this rendered form, so every merge diagnostic carries these strings.
std::string PresumedLocString(const clang::ASTContext& ctx,
                              clang::SourceLocation loc) {
  const clang::PresumedLoc presumed =
      ctx.getSourceManager().getPresumedLoc(loc);
  if (!presumed.isValid()) return "<unknown location>";
  return std::string(presumed.getFilename()) + ":" +
         std::to_string(presumed.getLine()) + ":" +
         std::to_string(presumed.getColumn());
}

std::string MainFilePath(const clang::ASTContext& ctx) {
  const clang::SourceManager& sm = ctx.getSourceManager();
  return std::string(
      sm.getFilename(sm.getLocForStartOfFile(sm.getMainFileID())));
}

// The definition's canonical path + file offset: the site identity behind
// the "one task, one definition site" rule. Distinct from the identity key
// on purpose -- a declaration and the definition it announces share a key
// but never a site, and the site is only compared between two definitions.
std::string DefinitionSite(const clang::FunctionDecl* func) {
  const clang::SourceManager& sm = func->getASTContext().getSourceManager();
  const clang::SourceLocation loc = sm.getExpansionLoc(func->getLocation());
  const clang::FileID file = sm.getFileID(loc);
  const clang::OptionalFileEntryRef entry = sm.getFileEntryRefForID(file);
  const std::string path = entry ? CanonicalPath(entry->getName())
                                 : CanonicalPath(sm.getFilename(loc));
  return path + ":" + std::to_string(sm.getFileOffset(loc));
}

// Canonical parameter-type signature: the identity half that makes a
// declaration and its definition (possibly in different TUs) compute the
// same key.
std::string CanonicalSignature(const clang::FunctionDecl* func) {
  clang::PrintingPolicy policy(func->getASTContext().getPrintingPolicy());
  policy.SuppressTagKeyword = true;
  std::string signature;
  for (unsigned i = 0; i < func->getNumParams(); ++i) {
    if (i > 0) signature += ",";
    signature +=
        func->getParamDecl(i)->getType().getCanonicalType().getAsString(policy);
  }
  return signature;
}

// Every function declaration written in the main file, in source order —
// declarations and definitions both; the builder keys them by identity.
// RecursiveASTVisitor does not descend into implicit template
// instantiations, so a specialization is sighted only through the invoke
// that instantiated it.
class MainFileFuncs : public clang::RecursiveASTVisitor<MainFileFuncs> {
 public:
  MainFileFuncs(const clang::ASTContext& ctx,
                const std::set<clang::FileID>& files)
      : ctx_(ctx), files_(files) {}

  bool VisitFunctionDecl(clang::FunctionDecl* func) {
    if (!IsInSourceFiles(ctx_, func->getLocation(), files_)) return true;
    if (!func->isImplicit() && !func->isDefaulted() && !func->isDeleted()) {
      funcs_.push_back(func);
    }
    return true;
  }

  std::vector<const clang::FunctionDecl*> TakeFuncs() {
    return std::move(funcs_);
  }

 private:
  const clang::ASTContext& ctx_;
  const std::set<clang::FileID>& files_;
  std::vector<const clang::FunctionDecl*> funcs_;
};

const char* LevelStr(TaskLevel level) {
  return level == TaskLevel::kUpper ? "upper" : "lower";
}

// Record `func` under `key` in a TU's visible-decl map: a definition
// outranks a bare declaration when both are visible.
void RecordVisibleDecl(
    std::map<std::string, const clang::FunctionDecl*>& visible,
    const std::string& key, const clang::FunctionDecl* func) {
  auto [it, inserted] = visible.try_emplace(key, func);
  if (!inserted && !it->second->isThisDeclarationADefinition() &&
      func->isThisDeclarationADefinition()) {
    it->second = func;
  }
}

// Maps the per-task synthesis policy to its wire string. tapa-ir's SynthTarget
// is closed over {"hls", "ignore", "aie"}, so both HLS and Vitis tasks
// collapse to "hls": the per-task field only says how to build the task. The
// flow itself is emitted once at the graph root (see FlowStr).
const char* SynthStr(SynthTarget target) {
  switch (target) {
    case SynthTarget::kIgnore:
      return "ignore";
    case SynthTarget::kAie:
      return "aie";
    case SynthTarget::kXilinxHls:
    case SynthTarget::kXilinxVitis:
    case SynthTarget::kXilinxSlash:
      return "hls";
  }
  // Unreachable for a valid enumerator; keeps -Wswitch (not -Wswitch-default)
  // free to flag a future enumerator that needs a policy decision here.
  return "hls";
}

// Wire string for the root-level vendor flow. Kebab-case, unlike the
// per-task policy above.
const char* FlowStr(SynthTarget default_target) {
  switch (default_target) {
    case SynthTarget::kXilinxHls:
      return "xilinx-hls";
    case SynthTarget::kXilinxVitis:
      return "xilinx-vitis";
    case SynthTarget::kXilinxSlash:
      return "xilinx-slash";
    case SynthTarget::kIgnore:
    case SynthTarget::kAie:
      break;
  }
  return "xilinx-hls";
}

nlohmann::json PortJson(const Port& port) {
  nlohmann::json j{{kFieldCat, TapaKindCat(port.kind)},
                   {kFieldName, port.name},
                   {kFieldType, port.ctype},
                   {kFieldWidth, port.width}};
  if (port.chan_count) j[kFieldChanCount] = *port.chan_count;
  if (port.chan_size) j[kFieldChanSize] = *port.chan_size;
  if (port.frame) j[kFieldFrame] = *port.frame;
  if (port.ring) j[kFieldRing] = *port.ring;
  if (port.aie_reads || port.aie_writes) {
    nlohmann::json& movers = j[kFieldAieMovers] = nlohmann::json::object();
    if (port.aie_reads) movers[kFieldReads] = *port.aie_reads;
    if (port.aie_writes) movers[kFieldWrites] = *port.aie_writes;
  }
  return j;
}

// The task serving one direction of an AI Engine task's async_mmap port.
// HLS folds a run of underscores in a top's name into one, so a mover's
// name never has two in a row.
std::string MoverName(const std::string& task, const std::string& port,
                      bool writes) {
  const std::string raw =
      "tapa_mover_" + task + "_" + port + (writes ? "_writes" : "_reads");
  std::string name;
  for (const char c : raw) {
    if (c == '_' && !name.empty() && name.back() == '_') continue;
    name += c;
  }
  return name;
}

// The ports of a mover (tapa/base/mover.h) serving one direction of the AI
// Engine task's async_mmap port `memory`: a write request and its data share
// the request channel.
std::vector<Port> MoverPorts(const Port& memory, bool writes, bool responses) {
  const std::string element = memory.ctype.substr(0, memory.ctype.size() - 1);
  const auto port = [](std::string name, TapaKind kind, std::string ctype,
                       uint32_t width) {
    return Port{std::move(name), kind, std::move(ctype), width};
  };
  std::vector<Port> ports{
      port("mem", TapaKind::kAsyncMmap, memory.ctype, memory.width)};
  if (!writes) {
    ports.push_back(port("requests", TapaKind::kIStream, "tapa::burst", 128));
    ports.push_back(port("data", TapaKind::kOStream, element, memory.width));
    return ports;
  }
  ports.push_back(port("requests", TapaKind::kIStream, element, memory.width));
  if (responses) {
    ports.push_back(port("responses", TapaKind::kOStream, "uint32_t", 32));
  }
  return ports;
}

nlohmann::json InstanceJson(const Instance& inst) {
  nlohmann::json j;
  j[kFieldStep] = inst.step;
  j[kFieldPosition] = inst.position;
  if (inst.name) j[kFieldName] = *inst.name;
  j[kFieldArgs] = nlohmann::json::object();
  for (const auto& [port, arg] : inst.args) {
    // A name serializes as a string; a constant as {width, value}, leaving
    // the Verilog spelling to the RTL backend.
    nlohmann::json bound = arg.value ? nlohmann::json{{kFieldWidth, arg.width},
                                                      {kFieldValue, *arg.value}}
                                     : nlohmann::json(arg.arg);
    j[kFieldArgs][port] = {{kFieldArg, std::move(bound)},
                           {kFieldCat, TapaKindCat(arg.cat)}};
  }
  return j;
}

nlohmann::json StreamJson(const StreamDecl& stream) {
  nlohmann::json j;
  // External top-level stream ports have no depth; the key is omitted so
  // tapa-ir parses the entry as an external (kernel-boundary) FIFO.
  if (stream.depth) j[kFieldDepth] = *stream.depth;
  if (stream.produced_by) {
    j[kFieldProducedBy] = {stream.produced_by->task, stream.produced_by->index};
  }
  if (stream.consumed_by) {
    j[kFieldConsumedBy] = {stream.consumed_by->task, stream.consumed_by->index};
  }
  return j;
}

// The TU id of `file` in the TU table, or -1 when never registered.
int FindTu(const std::vector<std::string>& tu_files, const std::string& file) {
  for (size_t i = 0; i < tu_files.size(); ++i) {
    if (tu_files[i] == file) return static_cast<int>(i);
  }
  return -1;
}

}  // namespace

ProgramBuilder::ProgramBuilder(std::string top, SynthTarget default_target,
                               TreeConfig tree)
    : top_(std::move(top)),
      default_target_(default_target),
      tree_config_(std::move(tree)) {
  tree_writer_ = std::make_unique<TreeWriter>(tree_config_.out_root,
                                              tree_config_.main_files);
}

ProgramBuilder::~ProgramBuilder() = default;
ProgramBuilder::ProgramBuilder(ProgramBuilder&&) noexcept = default;
ProgramBuilder& ProgramBuilder::operator=(ProgramBuilder&&) noexcept = default;

std::string ProgramBuilder::KeyOf(clang::MangleContext& mangler,
                                  const clang::FunctionDecl* func) const {
  if (const clang::FunctionDecl* definition = func->getDefinition()) {
    func = definition;
  }
  return func->isFunctionTemplateSpecialization()
             ? MangledTaskName(mangler, func)
             : func->getQualifiedNameAsString() + "(" +
                   CanonicalSignature(func) + ")";
}

ProgramBuilder::DefSighting ProgramBuilder::MakeFacts(
    clang::MangleContext& mangler, const clang::FunctionDecl* def,
    int tu) const {
  DefSighting sighting;
  const clang::ASTContext& ctx = def->getASTContext();
  sighting.tu = tu;
  sighting.fqn = def->getQualifiedNameAsString();
  sighting.plain_name = def->getNameAsString();
  sighting.signature = CanonicalSignature(def);
  for (const clang::ParmVarDecl* param : def->parameters()) {
    const unsigned index = param->getFunctionScopeIndex();
    sighting.parameter_names.push_back(param->getNameAsString());
    const TapaKind kind = ClassifyTapaType(param);
    if (IsMmapInterface(kind)) sighting.takes_memory = true;
    const bool stream =
        kind == TapaKind::kIStream || kind == TapaKind::kOStream;
    sighting.stream_params.push_back(stream);
    sighting.frame_params.push_back(
        IsFramesPort(kind) ||
        (stream && ParamAttr<clang::TapaFrameAttr>(def, index) != nullptr));
    if (const auto* ring = ParamAttr<clang::TapaRingAttr>(def, index)) {
      sighting.ring_slots[static_cast<int>(index)] = ring->getSlots();
      sighting.ring_locs[static_cast<int>(index)] =
          PresumedLocString(ctx, ring->getLocation());
    }
  }
  sighting.is_spec = def->isFunctionTemplateSpecialization();
  sighting.implicit_spec =
      sighting.is_spec &&
      def->getTemplateSpecializationKind() == clang::TSK_ImplicitInstantiation;
  sighting.pattern = def->isDependentContext();
  sighting.key = KeyOf(mangler, def);
  sighting.site = DefinitionSite(def);
  if (sighting.is_spec) {
    if (const clang::FunctionDecl* pattern =
            def->getTemplateInstantiationPattern()) {
      sighting.primary_key = KeyOf(mangler, pattern);
    }
  }
  sighting.name = TaskName(mangler, def);
  sighting.readable_name = ReadableTaskName(ctx, def);
  sighting.task_shaped = GetTapaTaskObject(def->getBody()) != nullptr;
  sighting.returns =
      !sighting.task_shaped && !sighting.pattern && CanReturn(def);
  sighting.ignored = IsIgnored(def);
  sighting.explicit_target = TargetAttr(def) != nullptr;
  for (const clang::FunctionDecl* decl : def->redecls()) {
    const auto* attr = decl->getAttr<clang::TapaTargetAttr>();
    if (attr != nullptr && attr->getTarget() != TargetAttr(def)->getTarget()) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                       attr->getLocation(),
                       "[[tapa::target]] of '%0' differs from the one at %1; "
                       "give every declaration of a task the same target")
          << sighting.fqn
          << PresumedLocString(ctx, TargetAttr(def)->getLocation());
    }
  }
  // isGlobal() holds for a static member function, which is no task either.
  sighting.internal = !def->isGlobal() || llvm::isa<clang::CXXMethodDecl>(def);
  sighting.level = LevelOf(def);
  sighting.target = ResolveTarget(def, default_target_);
  sighting.loc = PresumedLocString(ctx, def->getLocation());
  return sighting;
}

// The TU's visible declarations: identity key -> a decl of that function
// visible in this TU, a definition outranking a bare declaration. A
// template specialization is never RAV-visited, so it enters visibility
// through the invoke that instantiated it -- the invoke chain of every
// upper-shaped definition is walked for exactly that. This is all the
// rewrite pass resolves tasks through; the index pass needs the full
// sighting and edge index on top (IndexTuImpl).
std::map<std::string, const clang::FunctionDecl*> ProgramBuilder::VisibleDecls(
    clang::ASTContext& ctx, const std::set<clang::FileID>& files) const {
  std::map<std::string, const clang::FunctionDecl*> visible;
  const auto mangler = CreateMangleContext(ctx);
  MainFileFuncs collector(ctx, files);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(ctx.getTranslationUnitDecl());
  auto record = [&](const clang::FunctionDecl* func) {
    RecordVisibleDecl(visible, KeyOf(*mangler, func), func);
  };
  std::vector<const clang::FunctionDecl*> uppers;
  for (const clang::FunctionDecl* func : collector.TakeFuncs()) {
    record(func);
    if (func->isThisDeclarationADefinition() &&
        LevelOf(func) == TaskLevel::kUpper) {
      uppers.push_back(func);
    }
  }
  for (const clang::FunctionDecl* upper : uppers) {
    const clang::Expr* task_obj = GetTapaTaskObject(upper->getBody());
    for (const clang::CXXMemberCallExpr* invoke : GetInvokes(task_obj)) {
      const clang::FunctionDecl* callee = InvokeCallee(invoke);
      if (callee != nullptr && callee->isFunctionTemplateSpecialization()) {
        record(callee);
      }
    }
  }
  return visible;
}

ProgramBuilder::TuIndex ProgramBuilder::IndexTuImpl(
    clang::ASTContext& ctx, const std::set<clang::FileID>& files) const {
  TuIndex index;
  const auto mangler = CreateMangleContext(ctx);
  MainFileFuncs collector(ctx, files);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(ctx.getTranslationUnitDecl());
  const int tu = static_cast<int>(tu_files_.size());  // provisional id

  std::set<std::string> sighted;
  auto record_sighting = [&](const clang::FunctionDecl* func) {
    const std::string key = KeyOf(*mangler, func);
    RecordVisibleDecl(index.visible, key, func);
    if (!func->isThisDeclarationADefinition()) return;
    if (!sighted.insert(key).second) return;  // one sighting per key per TU
    index.defs.push_back(MakeFacts(*mangler, func, tu));
  };
  for (const clang::FunctionDecl* func : collector.TakeFuncs()) {
    record_sighting(func);
  }

  // Invoke edges, from every upper-shaped definition (discovery filters
  // them later; the callee is resolved through the declaration visible
  // here, so a cross-TU invoke sees only a declaration). A specialization
  // callee is also the specialization's only definition sighting in a TU
  // that never names it outside an invoke.
  for (size_t i = 0; i < index.defs.size(); ++i) {
    // By value: recording a specialization callee below can grow `defs`.
    const DefSighting caller = index.defs[i];
    if (caller.level != TaskLevel::kUpper) continue;
    const clang::FunctionDecl* def = index.visible.at(caller.key);
    const clang::Expr* task_obj = GetTapaTaskObject(def->getBody());
    for (const clang::CXXMemberCallExpr* invoke : GetInvokes(task_obj)) {
      const clang::FunctionDecl* callee = InvokeCallee(invoke);
      if (callee == nullptr) continue;
      const std::string callee_key = KeyOf(*mangler, callee);
      // A specialization is never RAV-visited, so it enters visibility and
      // the definition index through the invoke that instantiated it.
      record_sighting(callee);
      InvokeEdge edge;
      edge.caller_key = caller.key;
      edge.callee_key = callee_key;
      edge.callee_fqn = callee->getQualifiedNameAsString();
      edge.callee_name = callee->getNameAsString();
      edge.caller_name = caller.plain_name;
      edge.loc = PresumedLocString(ctx, invoke->getBeginLoc());
      for (const auto& [param, arg] : InvokePortArgs(invoke)) {
        const auto* ref =
            llvm::dyn_cast<clang::DeclRefExpr>(arg->IgnoreImplicit());
        const auto* own =
            ref == nullptr ? nullptr
                           : llvm::dyn_cast<clang::ParmVarDecl>(ref->getDecl());
        if (own != nullptr) {
          edge.bindings.emplace_back(
              static_cast<int>(param),
              static_cast<int>(own->getFunctionScopeIndex()));
        }
      }
      index.invokes.push_back(std::move(edge));
    }
  }
  return index;
}

int ProgramBuilder::RegisterTu(const std::string& file) {
  if (const int tu = FindTu(tu_files_, file); tu >= 0) return tu;
  tu_files_.push_back(file);
  return static_cast<int>(tu_files_.size() - 1);
}

int ProgramBuilder::TuOf(const std::string& file) const {
  const int tu = FindTu(tu_files_, file);
  if (tu < 0) {
    llvm::report_fatal_error(
        "ProgramBuilder: RewriteTreeTu called on a TU that IndexTu never saw");
  }
  return tu;
}

// The scanned-TU list every "found nothing anywhere" diagnostic carries.
std::string ProgramBuilder::ScannedTus() const {
  std::string scanned;
  for (const std::string& file : tu_files_) {
    if (!scanned.empty()) scanned += ", ";
    scanned += file;
  }
  return scanned;
}

void ProgramBuilder::Fail(std::string message) {
  errors_.push_back(std::move(message));
}

std::set<clang::FileID> ProgramBuilder::TreeFiles(
    clang::ASTContext& ctx, const std::vector<IncludeDirective>& log) const {
  std::set<clang::FileID> files;
  for (const auto& [path, file] :
       MirrorFiles(ctx, tree_config_.main_files, log)) {
    (void)path;
    files.insert(file);
  }
  return files;
}

void ProgramBuilder::IndexTu(clang::ASTContext& ctx,
                             const std::vector<IncludeDirective>& log) {
  const std::set<clang::FileID> files = TreeFiles(ctx, log);
  TuIndex index = IndexTuImpl(ctx, files);
  const int tu = RegisterTu(MainFilePath(ctx));
  for (DefSighting& sighting : index.defs) {
    sighting.tu = tu;
    defs_[sighting.key].push_back(std::move(sighting));
  }
  for (InvokeEdge& edge : index.invokes) {
    edge.tu = tu;
    edges_.emplace(edge.caller_key, std::move(edge));
  }
}

const ProgramBuilder::DefSighting& ProgramBuilder::FirstSighting(
    const std::string& key) const {
  return defs_.at(key).front();
}

void ProgramBuilder::SetTaskTargets(
    std::map<std::string, SynthTarget> targets) {
  task_targets_ = std::move(targets);
}

bool ProgramBuilder::ApplyTaskTargets() {
  // A `--task-target` mapping wins over the source attribute, so one source
  // builds both PL-only and PL+AI Engine. It is applied to every sighting
  // before any merge rule reads a task's level or target.
  for (const auto& [name, target] : task_targets_) {
    bool matched = false;
    for (auto& [key, sightings] : defs_) {
      for (DefSighting& sighting : sightings) {
        if (sighting.internal ||
            (sighting.plain_name != name && sighting.name != name &&
             sighting.readable_name != name)) {
          continue;
        }
        matched = true;
        sighting.target = target;
        sighting.explicit_target = true;
        sighting.ignored = target == SynthTarget::kIgnore;
        sighting.level = !sighting.ignored && sighting.task_shaped
                             ? TaskLevel::kUpper
                             : TaskLevel::kLower;
      }
    }
    if (!matched) {
      Fail("--target maps '" + name +
           "', but no task of that name is defined (scanned translation "
           "units: " +
           ScannedTus() + ")");
      return false;
    }
  }
  return true;
}

bool ProgramBuilder::PropagateAie(const std::string& top_key, bool aie_top) {
  // Recursive invocations are not a task tree. Diagnose them before target
  // propagation or recursive AIE flattening can loop forever.
  std::map<std::string, int> state;
  std::function<bool(const std::string&)> has_cycle =
      [&](const std::string& key) {
        state[key] = 1;
        for (auto [it, end] = edges_.equal_range(key); it != end; ++it) {
          const std::string& child = it->second.callee_key;
          if (state[child] == 1) return true;
          if (state[child] == 0 && has_cycle(child)) return true;
        }
        state[key] = 2;
        return false;
      };
  for (const auto& [key, task] : tasks_) {
    if (state[key] == 0 && has_cycle(key)) {
      Fail("task invocation cycle reaches '" + key + "'");
      return false;
    }
  }

  // An upper task's target is the default for its subtree: a descendant
  // with no target of its own inherits the AI Engine from the nearest AI
  // Engine ancestor, and one with an explicit target keeps it and hands it
  // down instead. An AI Engine top sets that default, and stays the PL
  // kernel, which owns the control interface, unless it needs none (below).
  auto aie_scope = [&](const std::string& key) {
    return (key == top_key && aie_top) ||
           (tasks_.at(key).model.target == SynthTarget::kAie &&
            tasks_.at(key).model.level == TaskLevel::kUpper);
  };
  std::queue<std::string> work;
  for (const auto& [key, task] : tasks_) {
    if (aie_scope(key)) work.push(key);
  }
  std::map<std::string, std::string> inherited_from;
  while (!work.empty()) {
    const std::string key = work.front();
    work.pop();
    for (auto [it, end] = edges_.equal_range(key); it != end; ++it) {
      const std::string& callee = it->second.callee_key;
      MergedTask& child = tasks_.at(callee);
      if (FirstSighting(callee).explicit_target ||
          child.model.target == SynthTarget::kAie) {
        continue;
      }
      child.model.target = SynthTarget::kAie;
      inherited_from.emplace(callee, tasks_.at(key).model.name);
      if (child.model.level == TaskLevel::kUpper) work.push(callee);
    }
  }
  // One task runs on one fabric: inheriting the AI Engine from one invoker
  // while a PL task also invokes it leaves no answer.
  for (const auto& [caller, edge] : edges_) {
    const auto inherited = inherited_from.find(edge.callee_key);
    if (inherited == inherited_from.end() ||
        tasks_.find(caller) == tasks_.end() || aie_scope(caller)) {
      continue;
    }
    Fail("task '" + tasks_.at(edge.callee_key).model.name +
         "' inherits the AI Engine from '" + inherited->second +
         "' but is also invoked by the PL task '" + edge.caller_name + "' at " +
         edge.loc + "; give it a target with [[tapa::target]] " +
         "or --target so it runs on one fabric");
    return false;
  }
  // An AI Engine top needs no PL kernel when no task below it runs on the
  // PL and it takes no memory, which only PL movers serve: it stays on the
  // AI Engine, its stream arguments GMIOs and its scalars runtime
  // parameters.
  const bool pl_below =
      std::any_of(tasks_.begin(), tasks_.end(), [&](const auto& entry) {
        return entry.first != top_key &&
               entry.second.model.target != SynthTarget::kAie;
      });
  if (aie_top && !pl_below && !FirstSighting(top_key).takes_memory) {
    tasks_.at(top_key).model.target = SynthTarget::kAie;
  }
  return true;
}

bool ProgramBuilder::ComputeRings(const std::string& top_key) {
  // Whether parameter `index` of the task `key` is a frame port, or reaches
  // one through the invokes that pass it on.
  std::map<std::pair<std::string, int>, bool> memo;
  std::function<bool(const std::string&, int)> reaches_frame =
      [&](const std::string& key, int index) {
        auto [it, inserted] = memo.emplace(std::make_pair(key, index), false);
        if (!inserted) return it->second;
        const DefSighting& def = FirstSighting(key);
        bool reaches = index < static_cast<int>(def.frame_params.size()) &&
                       def.frame_params[index];
        for (auto [edge, end] = edges_.equal_range(key);
             !reaches && edge != end; ++edge) {
          if (defs_.count(edge->second.callee_key) == 0) continue;
          for (const auto& [callee, caller] : edge->second.bindings) {
            if (caller == index &&
                reaches_frame(edge->second.callee_key, callee)) {
              reaches = true;
              break;
            }
          }
        }
        memo[{key, index}] = reaches;
        return reaches;
      };
  const auto top_frame_stream = [&](const std::string& key, int index) {
    const DefSighting& def = FirstSighting(key);
    return key == top_key && def.level == TaskLevel::kUpper &&
           def.stream_params[index] && reaches_frame(key, index);
  };
  for (const auto& [key, task] : tasks_) {
    const DefSighting& def = FirstSighting(key);
    for (const auto& [index, slots] : def.ring_slots) {
      if (!top_frame_stream(key, index)) {
        Fail("[[tapa::ring]] at " + def.ring_locs.at(index) + " is on '" +
             def.parameter_names[index] + "' of '" + def.name +
             "': only a stream argument of the top task that reaches a frame "
             "port has a ring");
        return false;
      }
      // The 32-bit frame counts wrap: the host takes slot `count % S` and
      // the RTL steps its slots round-robin, which agree across the wrap
      // only when S divides 2^32.
      if (slots == 0 || (slots & (slots - 1)) != 0) {
        Fail("[[tapa::ring(" + std::to_string(slots) + ")]] at " +
             def.ring_locs.at(index) +
             ": a ring holds a power-of-two number of frames, so its slots "
             "stay in step when the 32-bit frame count wraps");
        return false;
      }
    }
  }
  // A ring carries a stream argument of a PL top on XRT, where the host
  // moves its frames through device memory; on every other target the
  // argument stays a stream. An AI Engine top (see PropagateAie) gets its
  // rings in the PL kernel a platform whose graph only PL drives builds
  // for it; elsewhere its arguments are GMIOs and the rings go unused.
  MergedTask& top = tasks_.at(top_key);
  const bool vitis_kernel = top.model.target == SynthTarget::kXilinxVitis ||
                            (top.model.target == SynthTarget::kAie &&
                             default_target_ == SynthTarget::kXilinxVitis);
  if (!vitis_kernel) return true;
  const DefSighting& def = FirstSighting(top_key);
  for (int index = 0; index < static_cast<int>(def.stream_params.size());
       ++index) {
    if (!top_frame_stream(top_key, index)) continue;
    auto slots = def.ring_slots.find(index);
    top.model.rings[def.parameter_names[index]] =
        slots == def.ring_slots.end() ? 4 : slots->second;
  }
  return true;
}

bool ProgramBuilder::MergeAndDiscover() {
  if (!ApplyTaskTargets()) return false;

  // Rule 2: one key, one task. Sighted definitions must agree on
  // level/target/readable_name/signature; anything else is two different
  // functions wearing one identity. They must also sight the same graph:
  // invoke edges are recorded per sighting, and a shared upper definition
  // whose invokes are macro-selected per TU has identical facts on both
  // sightings -- and identical rewritten bytes, the upper body being a
  // shell -- so only comparing the sighted invoke structure keeps the
  // merge from silently unioning two different graphs.
  auto sighted_invokes = [this](const DefSighting& sighting) {
    std::vector<const InvokeEdge*> invokes;
    for (auto [it, end] = edges_.equal_range(sighting.key); it != end; ++it) {
      if (it->second.tu == sighting.tu) invokes.push_back(&it->second);
    }
    return invokes;
  };
  auto invoke_list = [](const std::vector<const InvokeEdge*>& invokes) {
    if (invokes.empty()) return std::string("no tasks");
    std::string list;
    for (const InvokeEdge* invoke : invokes) {
      if (!list.empty()) list += ", ";
      list += "'" + invoke->callee_name + "'";
    }
    return list;
  };
  for (const auto& [key, sightings] : defs_) {
    for (size_t i = 1; i < sightings.size(); ++i) {
      const DefSighting& first = sightings.front();
      const DefSighting& other = sightings[i];
      if (first.level != other.level || first.target != other.target ||
          first.readable_name != other.readable_name ||
          first.signature != other.signature) {
        Fail("task '" + first.name +
             "' is defined differently in two "
             "translation units: " +
             other.loc + " (" + tu_files_[other.tu] + ") and " + first.loc +
             " (" + tu_files_[first.tu] +
             "); a task sighted in several translation "
             "units must be identical in each");
        return false;
      }
      const std::vector<const InvokeEdge*> first_invokes =
          sighted_invokes(first);
      const std::vector<const InvokeEdge*> other_invokes =
          sighted_invokes(other);
      bool same_graph = first_invokes.size() == other_invokes.size();
      for (size_t e = 0; same_graph && e < first_invokes.size(); ++e) {
        same_graph =
            first_invokes[e]->callee_key == other_invokes[e]->callee_key;
      }
      if (!same_graph) {
        Fail("task '" + first.name +
             "' builds a different task graph in two "
             "translation units: " +
             other.loc + " (" + tu_files_[other.tu] + ") invokes " +
             invoke_list(other_invokes) + " but " + first.loc + " (" +
             tu_files_[first.tu] + ") invokes " + invoke_list(first_invokes) +
             "; a task sighted in several translation "
             "units must build the same task graph in each");
        return false;
      }
    }
  }

  // Rule 4 (task-shaped half): a task in an anonymous namespace, declared
  // `static`, or a member function can never be discovered; saying so at
  // the definition beats failing far away at link or elaboration time.
  for (const auto& [key, sightings] : defs_) {
    const DefSighting& sighting = sightings.front();
    if (sighting.internal && sighting.task_shaped) {
      Fail("'" + sighting.fqn + "' at " + sighting.loc +
           " builds a tapa::task() but is not an externally linked global "
           "function (static, anonymous namespace, or a member function), "
           "so it can never be a task; give it external linkage");
      return false;
    }
  }

  // Rule 5: the top resolves over every sighted definition. Internal
  // definitions are not candidates, exactly as discovery never was.
  std::vector<const DefSighting*> top_candidates;
  for (const auto& [key, sightings] : defs_) {
    const DefSighting& sighting = sightings.front();
    if (!sighting.internal && sighting.plain_name == top_) {
      top_candidates.push_back(&sighting);
    }
  }
  if (top_candidates.empty()) {
    Fail("top-level task '" + top_ +
         "' not found (scanned translation "
         "units: " +
         ScannedTus() + ")");
    return false;
  }
  if (top_candidates.size() > 1) {
    Fail("top-level task '" + top_ +
         "' has multiple definitions: " + top_candidates[0]->loc + " (" +
         tu_files_[top_candidates[0]->tu] + ") and " + top_candidates[1]->loc +
         " (" + tu_files_[top_candidates[1]->tu] + ")");
    return false;
  }
  const std::string top_key = top_candidates[0]->key;
  if (top_candidates[0]->pattern || top_candidates[0]->is_spec) {
    // A task is concrete (its ports need their types' widths), and the
    // kernel goes by the top's own name, which a specialization lacks.
    Fail("top-level task '" + top_ + "' (defined at " + top_candidates[0]->loc +
         ") is a template; the top must be an ordinary function");
    return false;
  }
  if (top_candidates[0]->ignored) {
    Fail("tapa top-level task function cannot be ignored (defined at " +
         top_candidates[0]->loc + ")");
    return false;
  }
  // An AI Engine top is the default for the tasks it invokes; the top
  // itself is the PL kernel unless it needs none (see PropagateAie). A leaf
  // top has nothing to hand the default to, and the PL kernel owns the
  // control interface.
  const bool aie_top = top_candidates[0]->target == SynthTarget::kAie;
  if (aie_top && top_candidates[0]->level != TaskLevel::kUpper) {
    Fail("top-level task '" + top_ + "' (defined at " + top_candidates[0]->loc +
         ") is a leaf and cannot run on the AI Engine: the PL kernel owns the "
         "control interface, so invoke the AI Engine work from an upper top");
    return false;
  }

  // Discovery: breadth-first over the merged invoke edges.
  std::queue<std::string> work;
  auto discover = [&](const DefSighting& sighting,
                      std::optional<std::string> invoker_key) {
    auto [it, inserted] = tasks_.emplace(sighting.key, MergedTask{});
    if (!inserted) return;
    MergedTask& task = it->second;
    task.owner_tu = sighting.tu;
    task.invoker_key = std::move(invoker_key);
    task.primary_key = sighting.primary_key;
    task.model.is_template_spec = sighting.is_spec;
    task.model.name = sighting.name;
    task.model.readable_name = sighting.readable_name;
    task.model.level = sighting.level;
    task.model.target = sighting.target;
    work.push(sighting.key);
  };
  discover(*top_candidates[0], std::nullopt);
  if (aie_top) tasks_.at(top_key).model.target = default_target_;
  while (!work.empty()) {
    const std::string key = work.front();
    work.pop();
    const MergedTask& task = tasks_.at(key);
    // Only an upper task's body holds invokes to descend into; an ignored
    // one is lower-level by definition.
    if (task.model.level != TaskLevel::kUpper) continue;
    for (auto [edge_it, end] = edges_.equal_range(key); edge_it != end;
         ++edge_it) {
      const InvokeEdge& edge = edge_it->second;
      auto found = defs_.find(edge.callee_key);
      // Rule 1: every invoke must resolve to a definition somewhere.
      if (found == defs_.end()) {
        std::string message =
            "task '" + edge.callee_name + "' is invoked by '" +
            edge.caller_name + "' at " + edge.loc +
            " but has no definition in any scanned translation unit (" +
            ScannedTus() + ")";
        for (const auto& [key2, sightings] : defs_) {
          if (sightings.front().fqn == edge.callee_fqn) {
            message +=
                "; a function with the same name but a different "
                "signature is defined at " +
                sightings.front().loc + " (" + tu_files_[sightings.front().tu] +
                ")";
            break;
          }
        }
        Fail(std::move(message));
        return false;
      }
      // Rule 4 (invoked half).
      const DefSighting& callee = found->second.front();
      if (callee.internal) {
        Fail("task '" + callee.name + "' invoked by '" + edge.caller_name +
             "' at " + edge.loc +
             " is not an externally linked free function (static, "
             "anonymous namespace, or a member function); tasks must be");
        return false;
      }
      discover(callee,
               callee.is_spec ? std::optional<std::string>(key) : std::nullopt);
    }
  }
  // A mapping must name a task of this graph: one naming a helper or a
  // task the top never reaches would otherwise be ignored, typo or not.
  for (const auto& mapping : task_targets_) {
    const std::string& name = mapping.first;
    const bool reached =
        std::any_of(tasks_.begin(), tasks_.end(), [&](const auto& entry) {
          const DefSighting& sighting = FirstSighting(entry.first);
          return sighting.plain_name == name || sighting.name == name ||
                 sighting.readable_name == name;
        });
    if (!reached) {
      Fail("--target maps '" + name +
           "', but no task of that name runs "
           "under the top-level task '" +
           top_ + "'");
      return false;
    }
  }
  if (!PropagateAie(top_key, aie_top)) return false;
  if (!ComputeRings(top_key)) return false;

  // Vitis HLS renames a top function whose name begins with '_' (`_Fill`
  // becomes module `p_Fill`), so the task's RTL and reports never appear
  // under its name; mangled task names carry a prefix for the same reason.
  for (const auto& [key, task] : tasks_) {
    const TaskModel& model = task.model;
    if (model.name.empty() || model.name.front() != '_' ||
        model.target == SynthTarget::kAie ||
        model.target == SynthTarget::kIgnore) {
      continue;
    }
    Fail("task '" + model.name + "' (defined at " + FirstSighting(key).loc +
         ") begins with '_': Vitis HLS renames such a module (to 'p_...'), so "
         "its RTL would never appear under the task's name; rename the task");
    return false;
  }

  // Rule 2, site half: the mirror writes one file per source path, so a
  // task sighted at two distinct sites would need two different rewrites of
  // the same task. A header task sighted by several TUs sits at one site
  // and dedupes; an implicit specialization is sighted at its point of
  // instantiation, which is per-TU by construction and carries no second
  // definition.
  for (const auto& [key, task] : tasks_) {
    const DefSighting& first = FirstSighting(key);
    for (const DefSighting& other : defs_.at(key)) {
      if (other.implicit_spec || other.internal) continue;
      if (other.site == first.site) continue;
      Fail("task '" + first.name + "' is defined at two locations: " +
           first.loc + " (" + tu_files_[first.tu] + ") and " + other.loc +
           " (" + tu_files_[other.tu] +
           "); a task must be defined in exactly one place (a task defined "
           "in a header shared by several translation units is one "
           "definition and merges silently)");
      return false;
    }
  }

  // The redefinition guard over the merged index: two definitions with one
  // plain name make the name-keyed task map ambiguous. A specialization,
  // implicit or explicit, never counts (its task goes by its mangled name,
  // next to the template it specializes), and neither does an internal
  // helper.
  std::map<std::string, std::vector<const DefSighting*>> by_plain_name;
  for (const auto& [key, sightings] : defs_) {
    const DefSighting& sighting = sightings.front();
    if (sighting.internal || sighting.is_spec) continue;
    by_plain_name[sighting.plain_name].push_back(&sighting);
  }
  for (const auto& [key, task] : tasks_) {
    const std::string plain_name = FirstSighting(key).plain_name;
    const auto it = by_plain_name.find(plain_name);
    if (it == by_plain_name.end()) continue;
    if (it->second.size() > 1) {
      const DefSighting* a = it->second[0];
      const DefSighting* b = it->second[1];
      Fail("task '" + plain_name + "' re-defined: " + b->loc + " (" +
           tu_files_[b->tu] + ") and " + a->loc + " (" + tu_files_[a->tu] +
           "); task names must be unique across the "
           "program");
      return false;
    }
  }

  std::vector<std::string> task_keys;
  task_keys.reserve(tasks_.size());
  for (const auto& [key, task] : tasks_) {
    task_keys.push_back(task.model.name);
  }
  if (const std::optional<std::string> collision =
          TaskGuardCollision(task_keys)) {
    Fail(*collision);
    return false;
  }
  return true;
}

Program ProgramBuilder::TuView(clang::ASTContext& ctx,
                               const std::vector<IncludeDirective>& log) {
  const std::set<clang::FileID> files = TreeFiles(ctx, log);
  const std::map<std::string, const clang::FunctionDecl*> visible =
      VisibleDecls(ctx, files);
  Program view;
  view.top = top_;
  view.file_funcs = CollectFileFuncs(ctx, files);
  view.local_funcs = CollectLocalFuncs(ctx, files);
  for (const auto& [key, task] : tasks_) {
    // This TU's best decl for the task: the definition when it is visible
    // here, else a visible redeclaration (the shared header announcing a
    // task another TU defines). A template specialization is only declared
    // by its point of instantiation, so a TU that never instantiates it
    // resolves through the primary template instead: the shared header
    // defining that pattern must carry the same task guard in every TU
    // that includes it.
    const clang::FunctionDecl* def = nullptr;
    if (auto visible_it = visible.find(key); visible_it != visible.end()) {
      def = visible_it->second;
    } else if (task.model.is_template_spec && !task.primary_key.empty()) {
      if (auto primary_it = visible.find(task.primary_key);
          primary_it != visible.end()) {
        def = primary_it->second;
      }
    }
    if (def == nullptr) continue;
    TaskModel model;
    model.def = def;
    model.is_template_spec = task.model.is_template_spec;
    model.name = task.model.name;
    model.readable_name = task.model.readable_name;
    model.level = task.model.level;
    model.target = task.model.target;
    model.rings = task.model.rings;
    // The mangled wrapper is spliced after its invoker's definition; a TU
    // holding only the invoker's declaration must not splice anything into
    // it, or the header holding that declaration would render differently
    // than in the TU that owns the definition.
    if (task.invoker_key) {
      if (auto invoker_it = visible.find(*task.invoker_key);
          invoker_it != visible.end() &&
          invoker_it->second->isThisDeclarationADefinition()) {
        model.invoker = invoker_it->second;
      }
    }
    view.tasks.emplace(model.name, std::move(model));
  }
  return view;
}

void ProgramBuilder::FillOwnedTasks(int tu, Program& view,
                                    clang::ASTContext& ctx) {
  const auto mangler = CreateMangleContext(ctx);
  const auto parameter_name = [&](const clang::FunctionDecl* callee,
                                  unsigned index) {
    return FirstSighting(KeyOf(*mangler, callee)).parameter_names.at(index);
  };
  // A PL leaf whose body can return, which hardware runs once per invocation
  // of its parent when detached.
  const auto returning_pl_leaf = [&](const clang::FunctionDecl* callee) {
    const std::string key = KeyOf(*mangler, callee);
    const auto task = tasks_.find(key);
    return task != tasks_.end() &&
           task->second.model.level == TaskLevel::kLower &&
           task->second.model.target != SynthTarget::kAie &&
           task->second.model.target != SynthTarget::kIgnore &&
           FirstSighting(key).returns;
  };
  std::vector<MergedTask> movers;
  for (auto& [key, task] : tasks_) {
    if (task.owner_tu != tu) continue;
    TaskModel& model = view.tasks.at(task.model.name);
    // Ports, instances, and streams exist only where the definition is.
    model.ports = BuildPorts(ctx, model.def);
    ReadFrameAttrs(ctx, model);
    for (Port& port : model.ports) {
      if (auto ring = model.rings.find(port.name); ring != model.rings.end()) {
        port.ring = ring->second;
      }
    }
    if (model.target == SynthTarget::kAie) {
      CheckAieTask(ctx, model);
      if (model.level == TaskLevel::kLower) {
        model.aie_stack_bytes = LocalBytes(ctx, model.def);
        AddMovers(tu, model, view, movers);
      }
    } else if (model.target != SynthTarget::kIgnore) {
      model.closed_ports = ClosedPorts(model.def);
    }
    // An ignored task's body is only its software model, free to invoke.
    if (model.target != SynthTarget::kIgnore) {
      DiagnoseStrayInvokes(ctx, model.def);
    }
    if (model.level == TaskLevel::kUpper) {
      ParseUpperTask(ctx, model, /*is_top=*/model.name == top_, parameter_name,
                     returning_pl_leaf);
    }
    task.model = model;
  }
  for (MergedTask& mover : movers) {
    tasks_.emplace(mover.model.name, std::move(mover));
  }
}

void ProgramBuilder::AddMovers(int tu, TaskModel& model, Program& view,
                               std::vector<MergedTask>& movers) const {
  for (const clang::ParmVarDecl* param : model.def->parameters()) {
    if (!IsAsyncMmap(ClassifyTapaType(param))) continue;
    const std::string name = param->getNameAsString();
    auto port = std::find_if(model.ports.begin(), model.ports.end(),
                             [&](const Port& p) { return p.name == name; });
    if (port == model.ports.end()) continue;
    const MemoryUse use = UsedDirections(model.def, param);
    for (const bool writes : {false, true}) {
      if (!(writes ? use.writes : use.reads)) continue;
      MergedTask mover;
      mover.owner_tu = tu;
      mover.model.name = MoverName(model.name, name, writes);
      mover.model.readable_name = mover.model.name;
      mover.model.level = TaskLevel::kLower;
      mover.model.target = default_target_;
      const bool responses = writes && use.responses;
      mover.model.ports = MoverPorts(*port, writes, responses);
      (writes ? port->aie_writes : port->aie_reads) = mover.model.name;
      view.movers.push_back(Mover{mover.model.name, writes, responses,
                                  ElementTypeName(param), model.def});
      movers.push_back(std::move(mover));
    }
  }
}

void ProgramBuilder::RewriteTreeTu(
    clang::ASTContext& ctx, std::vector<IncludeDirective> log,
    const clang::syntax::TokenBuffer* tokens,
    const std::vector<clang::SourceRange>& skipped,
    const std::vector<ExpansionPragma>& pragmas) {
  const int tu = TuOf(MainFilePath(ctx));
  Program view = TuView(ctx, log);
  FillOwnedTasks(tu, view, ctx);

  const XilinxBackend hls(/*is_vitis=*/false);
  const XilinxBackend vitis(/*is_vitis=*/true);
  const IgnoreBackend ignore;
  const AieBackend aie;
  TreeSession session(ctx, log, tree_config_.main_files, tokens, pragmas);
  RewriteTreeFiles(view, default_target_, Backends{hls, vitis, ignore, aie},
                   ctx, session);
  StripSkippedAttrs(skipped, TreeFiles(ctx, log), ctx, session.edits());
  session.FlushMacroSplices();

  std::string error;
  if (!tree_writer_->AddTu(ctx, std::move(log), session, &error)) {
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                     ctx.getSourceManager().getLocForStartOfFile(
                         ctx.getSourceManager().getMainFileID()),
                     "rewritten-tree emission failed: %0")
        << error;
    return;
  }
  for (const auto& [path, buffer] : session.buffers()) {
    ReportLeakedAttrs(buffer->Render(), path, ctx, reported_leaks_);
  }
}

bool ProgramBuilder::CheckAieEot() {
  // A PL task that closes a stream feeding an AI Engine task would put an
  // end-of-transmission token on it; the AI Engine reads that as data.
  std::map<std::string, const TaskModel*> by_name;
  for (const auto& [key, task] : tasks_) {
    by_name.emplace(task.model.name, &task.model);
  }
  std::map<const TaskModel*, std::set<std::string>> close_cache;
  std::function<std::set<std::string>(const TaskModel&)> closed_ports =
      [&](const TaskModel& task) -> std::set<std::string> {
    if (const auto it = close_cache.find(&task); it != close_cache.end()) {
      return it->second;
    }
    std::set<std::string> closed = task.closed_ports;
    for (const auto& [child_name, instances] : task.instances) {
      const auto child = by_name.find(child_name);
      if (child == by_name.end()) continue;
      for (const auto& instance : instances) {
        const auto child_closed = closed_ports(*child->second);
        for (const auto& [port, arg] : instance.args) {
          const std::string child_port = ArrayBaseName(port);
          if (child_closed.count(child_port) == 0 || arg.arg.empty()) continue;
          const std::string upper_port = ArrayBaseName(arg.arg);
          if (FindPort(task, upper_port) != nullptr) closed.insert(upper_port);
        }
      }
    }
    return close_cache[&task] = std::move(closed);
  };
  // The AI Engine task a stream bound to `port` of `task` reaches, the task
  // itself or one a PL upper task hands the port to, or empty.
  std::map<std::pair<const TaskModel*, std::string>, std::string> aie_cache;
  std::function<std::string(const TaskModel&, const std::string&)> aie_reader =
      [&](const TaskModel& task, const std::string& port) -> std::string {
    if (task.target == SynthTarget::kAie) return task.name;
    const auto key = std::make_pair(&task, port);
    if (const auto it = aie_cache.find(key); it != aie_cache.end()) {
      return it->second;
    }
    std::string reader;
    for (const auto& [child_name, instances] : task.instances) {
      const auto child = by_name.find(child_name);
      if (child == by_name.end()) continue;
      for (const auto& instance : instances) {
        for (const auto& [child_port, arg] : instance.args) {
          if (reader.empty() && !arg.arg.empty() &&
              ArrayBaseName(arg.arg) == port) {
            reader = aie_reader(*child->second, ArrayBaseName(child_port));
          }
        }
      }
    }
    return aie_cache[key] = reader;
  };
  for (const auto& [key, task] : tasks_) {
    const TaskModel& upper = task.model;
    for (const auto& [fifo, stream] : upper.streams) {
      if (!stream.produced_by || !stream.consumed_by) continue;
      const auto producer = by_name.find(stream.produced_by->task);
      const auto consumer = by_name.find(stream.consumed_by->task);
      if (producer == by_name.end() || consumer == by_name.end()) continue;
      const auto readers = upper.instances.find(consumer->first);
      if (readers == upper.instances.end() ||
          stream.consumed_by->index >= readers->second.size()) {
        continue;
      }
      std::string reader;
      for (const auto& [port, arg] :
           readers->second[stream.consumed_by->index].args) {
        if (reader.empty() && arg.arg == fifo) {
          reader = aie_reader(*consumer->second, ArrayBaseName(port));
        }
      }
      if (reader.empty()) continue;
      const auto instances = upper.instances.find(producer->first);
      if (instances == upper.instances.end() ||
          stream.produced_by->index >= instances->second.size()) {
        continue;
      }
      for (const auto& [port, arg] :
           instances->second[stream.produced_by->index].args) {
        if (arg.arg != fifo) continue;
        const std::string base = ArrayBaseName(port);
        if (closed_ports(*producer->second).count(base) == 0) continue;
        Fail("task '" + producer->first + "' closes its stream '" + base +
             "', which feeds the AI Engine task '" + reader + "' through '" +
             fifo + "' in '" + upper.name +
             "'; AI Engine streams carry no end-of-transmission token, so "
             "send an element count instead of closing the stream");
        return false;
      }
    }
  }
  return true;
}

const TaskModel* ProgramBuilder::FindTask(const std::string& name) const {
  for (const auto& [key, task] : tasks_) {
    if (task.model.name == name) return &task.model;
  }
  return nullptr;
}

bool ProgramBuilder::WriteTree(std::string* error) {
  return tree_writer_->Write(error);
}

nlohmann::json ProgramBuilder::EmitJson() const {
  // Sorted by graph name, exactly as the single-TU std::map ordered tasks.
  std::map<std::string, const MergedTask*> by_name;
  for (const auto& [key, task] : tasks_) {
    by_name.emplace(task.model.name, &task);
  }

  nlohmann::json tree_srcs = nlohmann::json::array();
  for (const std::string& src : tree_writer_->MainFileKeys()) {
    tree_srcs.push_back(src);
  }
  nlohmann::json tree_include_dirs = nlohmann::json::array();
  tree_include_dirs.push_back("");
  for (const std::string& dir : tree_writer_->ExternalBuckets()) {
    tree_include_dirs.push_back(dir);
  }

  nlohmann::json out;
  out[kFieldSchemaVersion] = kSchemaVersion;
  out[kFieldTop] = top_;
  out[kFieldTarget] = FlowStr(default_target_);
  out[kFieldTasks] = nlohmann::json::object();
  for (const auto& [name, task] : by_name) {
    const TaskModel& model = task->model;
    nlohmann::json& t = out[kFieldTasks][name];
    // Every task compiles the same mirrored TUs; one define selects its
    // full definition while every other task takes its exact signature stub.
    t[kFieldSrcs] = tree_srcs;
    t[kFieldIncludeDirs] = tree_include_dirs;
    t[kFieldDefines] = nlohmann::json::array({TaskGuardDefine(model.name)});
    if (model.target == SynthTarget::kAie && !model.rings.empty()) {
      t[kFieldPlKernelDefines] =
          nlohmann::json::array({PlKernelGuardDefine(model.name)});
    }
    t[kFieldLevel] = LevelStr(model.level);
    t[kFieldSynth] = SynthStr(model.target);
    t[kFieldReadableName] = model.readable_name;
    t[kFieldPorts] = nlohmann::json::array();
    for (const Port& port : model.ports) {
      t[kFieldPorts].push_back(PortJson(port));
    }
    if (model.aie_stack_bytes > 0) {
      t[kFieldAieStackBytes] = model.aie_stack_bytes;
    }
    if (model.level == TaskLevel::kUpper) {
      t[kFieldTasks] = nlohmann::json::object();
      for (const auto& [child, instances] : model.instances) {
        nlohmann::json arr = nlohmann::json::array();
        for (const Instance& inst : instances) {
          arr.push_back(InstanceJson(inst));
        }
        t[kFieldTasks][child] = std::move(arr);
      }
      t[kFieldFifos] = nlohmann::json::object();
      for (const auto& [fifo, stream] : model.streams) {
        t[kFieldFifos][fifo] = StreamJson(stream);
      }
    }
  }
  return out;
}

}  // namespace tapa::cc
