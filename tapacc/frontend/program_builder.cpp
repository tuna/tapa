#include "program_builder.h"

#include <queue>
#include <set>

#include "clang/AST/Mangle.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/SourceManager.h"
#include "llvm/Support/ErrorHandling.h"

#include "build_program.h"
#include "classify.h"
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
                const std::set<clang::FileID>* files)
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
  const std::set<clang::FileID>* files_;
  std::vector<const clang::FunctionDecl*> funcs_;
};

const char* LevelStr(TaskLevel level) {
  return level == TaskLevel::kUpper ? "upper" : "lower";
}

// Maps the per-task synthesis policy to its wire string. tapa-ir's SynthTarget
// is closed over {"hls", "ignore"}, so both HLS and Vitis tasks collapse to
// "hls": the per-task field only says whether to synthesize. The flow itself
// is emitted once at the graph root (see FlowStr).
const char* SynthStr(SynthTarget target) {
  switch (target) {
    case SynthTarget::kIgnore:
      return "ignore";
    case SynthTarget::kXilinxHls:
    case SynthTarget::kXilinxVitis:
      return "hls";
  }
  // Unreachable for a valid enumerator; keeps -Wswitch (not -Wswitch-default)
  // free to flag a future enumerator that needs a policy decision here.
  return "hls";
}

// Wire string for the root-level vendor flow. Kebab-case, unlike the
// per-task policy above.
const char* FlowStr(SynthTarget default_target) {
  return default_target == SynthTarget::kXilinxVitis ? "xilinx-vitis"
                                                     : "xilinx-hls";
}

nlohmann::json PortJson(const Port& port) {
  nlohmann::json j{{kFieldCat, TapaKindCat(port.kind)},
                   {kFieldName, port.name},
                   {kFieldType, port.ctype},
                   {kFieldWidth, port.width}};
  if (port.chan_count) j[kFieldChanCount] = *port.chan_count;
  if (port.chan_size) j[kFieldChanSize] = *port.chan_size;
  return j;
}

nlohmann::json InstanceJson(const Instance& inst) {
  nlohmann::json j;
  j[kFieldStep] = inst.step;
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
  sighting.is_spec = def->isFunctionTemplateSpecialization();
  sighting.implicit_spec =
      sighting.is_spec &&
      def->getTemplateSpecializationKind() == clang::TSK_ImplicitInstantiation;
  sighting.key = KeyOf(mangler, def);
  sighting.site = DefinitionSite(def);
  if (sighting.is_spec) {
    if (const clang::FunctionDecl* pattern =
            def->getTemplateInstantiationPattern()) {
      sighting.primary_key = KeyOf(mangler, pattern);
    }
  }
  sighting.name =
      sighting.is_spec ? MangledTaskName(mangler, def) : sighting.plain_name;
  sighting.readable_name = ReadableTaskName(ctx, def);
  sighting.task_shaped = GetTapaTaskObject(def->getBody()) != nullptr;
  sighting.ignored = IsIgnored(def);
  sighting.internal = !def->isGlobal();
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
    clang::ASTContext& ctx, const std::set<clang::FileID>* files) const {
  std::map<std::string, const clang::FunctionDecl*> visible;
  const auto mangler = CreateMangleContext(ctx);
  MainFileFuncs collector(ctx, files);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(ctx.getTranslationUnitDecl());
  auto record = [&](const clang::FunctionDecl* func) {
    auto [it, inserted] = visible.try_emplace(KeyOf(*mangler, func), func);
    if (!inserted && !it->second->isThisDeclarationADefinition() &&
        func->isThisDeclarationADefinition()) {
      it->second = func;
    }
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
    clang::ASTContext& ctx, const std::set<clang::FileID>* files) const {
  TuIndex index;
  const auto mangler = CreateMangleContext(ctx);
  MainFileFuncs collector(ctx, files);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(ctx.getTranslationUnitDecl());
  const int tu = static_cast<int>(tu_files_.size());  // provisional id

  // Visible: identity key -> a decl of that function in this TU, a
  // definition outranking a bare declaration.
  std::set<std::string> sighted;
  auto record_visible = [&index](const std::string& key,
                                 const clang::FunctionDecl* func) {
    auto [it, inserted] = index.visible.try_emplace(key, func);
    if (!inserted && !it->second->isThisDeclarationADefinition() &&
        func->isThisDeclarationADefinition()) {
      it->second = func;
    }
  };
  auto record_sighting = [&](const clang::FunctionDecl* func) {
    const std::string key = KeyOf(*mangler, func);
    record_visible(key, func);
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
      index.invokes.push_back(std::move(edge));
    }
  }
  return index;
}

int ProgramBuilder::RegisterTu(const std::string& file) {
  for (size_t i = 0; i < tu_files_.size(); ++i) {
    if (tu_files_[i] == file) return static_cast<int>(i);
  }
  tu_files_.push_back(file);
  return static_cast<int>(tu_files_.size() - 1);
}

int ProgramBuilder::TuOf(const std::string& file) const {
  for (size_t i = 0; i < tu_files_.size(); ++i) {
    if (tu_files_[i] == file) return static_cast<int>(i);
  }
  llvm::report_fatal_error(
      "ProgramBuilder: RewriteTreeTu called on a TU that IndexTu never saw");
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
  TuIndex index = IndexTuImpl(ctx, &files);
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

bool ProgramBuilder::MergeAndDiscover() {
  // Rule 2: one key, one task. Sighted definitions must agree on
  // level/target/readable_name/signature; anything else is two different
  // functions wearing one identity.
  for (const auto& [key, sightings] : defs_) {
    for (size_t i = 1; i < sightings.size(); ++i) {
      const DefSighting& first = sightings.front();
      const DefSighting& other = sightings[i];
      if (first.level == other.level && first.target == other.target &&
          first.readable_name == other.readable_name &&
          first.signature == other.signature) {
        continue;
      }
      Fail("task '" + first.name +
           "' is defined differently in two "
           "translation units: " +
           other.loc + " (" + tu_files_[other.tu] + ") and " + first.loc +
           " (" + tu_files_[first.tu] +
           "); a task sighted in several translation "
           "units must be identical in each");
      return false;
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
  if (top_candidates[0]->ignored) {
    Fail("tapa top-level task function cannot be ignored (defined at " +
         top_candidates[0]->loc + ")");
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
             " has internal linkage (static or "
             "anonymous namespace); tasks must have external linkage");
        return false;
      }
      discover(callee,
               callee.is_spec ? std::optional<std::string>(key) : std::nullopt);
    }
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

  // The old single-TU redefinition guard, stated over the merged index:
  // two definitions with one plain name make the name-keyed task map
  // ambiguous. Implicit specializations never counted (they are sighted
  // only through invokes) and neither did internal helpers.
  std::map<std::string, std::vector<const DefSighting*>> by_plain_name;
  for (const auto& [key, sightings] : defs_) {
    const DefSighting& sighting = sightings.front();
    if (sighting.internal || sighting.implicit_spec) continue;
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
      VisibleDecls(ctx, &files);
  Program view;
  view.top = top_;
  view.file_funcs = CollectFileFuncs(ctx, &files);
  view.local_funcs = CollectLocalFuncs(ctx, &files);
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
  for (auto& [key, task] : tasks_) {
    if (task.owner_tu != tu) continue;
    TaskModel& model = view.tasks.at(task.model.name);
    // Ports, instances, and streams exist only where the definition is.
    model.ports = BuildPorts(ctx, model.def);
    if (model.level == TaskLevel::kUpper) {
      ParseUpperTask(ctx, model, /*is_top=*/model.name == top_);
    }
    task.model = model;
  }
}

void ProgramBuilder::RewriteTreeTu(clang::ASTContext& ctx,
                                   std::vector<IncludeDirective> log,
                                   const clang::syntax::TokenBuffer* tokens) {
  const int tu = TuOf(MainFilePath(ctx));
  Program view = TuView(ctx, log);
  FillOwnedTasks(tu, view, ctx);

  const XilinxBackend hls(/*is_vitis=*/false);
  const XilinxBackend vitis(/*is_vitis=*/true);
  const IgnoreBackend ignore;
  TreeSession session(ctx, log, tree_config_.main_files, tokens);
  RewriteTreeFiles(view, default_target_, hls, vitis, ignore, ctx, session);
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
    ReportLeakedAttrs(buffer->Render(), path, ctx);
  }
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
    t[kFieldLevel] = LevelStr(model.level);
    t[kFieldSynth] = SynthStr(model.target);
    t[kFieldReadableName] = model.readable_name;
    t[kFieldPorts] = nlohmann::json::array();
    for (const Port& port : model.ports) {
      t[kFieldPorts].push_back(PortJson(port));
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
