#include "rewrite.h"

#include <set>
#include <string>

#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/DeclTemplate.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/StmtCXX.h"
#include "clang/Basic/SourceManager.h"
#include "edit_sink.h"
#include "llvm/ADT/StringRef.h"

#include "conventions.h"
#include "emit.h"
#include "macro_splice.h"
#include "tree_writer.h"
#include "wrapper.h"

#include "frontend/diag.h"
#include "frontend/discover.h"

namespace tapa::cc {

namespace {

// The body of a loop statement, or nullptr if it is not a loop.
const clang::Stmt* GetLoopBody(const clang::Stmt* stmt) {
  if (const auto* s = llvm::dyn_cast<clang::DoStmt>(stmt)) return s->getBody();
  if (const auto* s = llvm::dyn_cast<clang::ForStmt>(stmt)) return s->getBody();
  if (const auto* s = llvm::dyn_cast<clang::WhileStmt>(stmt)) {
    return s->getBody();
  }
  if (const auto* s = llvm::dyn_cast<clang::CXXForRangeStmt>(stmt)) {
    return s->getBody();
  }
  return nullptr;
}

// The braces an attribute on a non-loop statement should scope to.
//
// A vendor region pragma is written INSIDE the braces it constrains, so
// `if (c) { #pragma HLS latency max = 1 ... }` migrates to an attribute on
// the `if`. Emitting that pragma *before* the `if` hands it to the enclosing
// region instead, silently widening the constraint.
const clang::Stmt* GetRegionBody(const clang::Stmt* stmt) {
  if (const auto* s = llvm::dyn_cast<clang::IfStmt>(stmt)) {
    // The one shape a vendor region pragma written inside the then-braces
    // migrates to. With an else, the pragma would cover only one branch
    // with nothing saying so; without braces, the pragma would land before
    // the `if` and constrain the ENCLOSING region. Both are diagnosed at
    // the lowering site, not silently reinterpreted.
    if (s->getElse() == nullptr &&
        llvm::isa<clang::CompoundStmt>(s->getThen())) {
      return s->getThen();
    }
    return nullptr;
  }
  if (llvm::isa<clang::CompoundStmt>(stmt)) return stmt;
  return nullptr;
}

// Listed explicitly rather than as a range over the generated AttrKinds:
// TableGen orders attr::Kind by record name within an inheritance group, so
// `TapaAggregate..TapaUnroll` covers today's thirteen only by accident of
// spelling. A future attribute sorting outside those endpoints — or one
// derived from InheritableAttr, which moves it to another group — would
// silently not be a TAPA attribute here, so LowerAttrs would skip it and
// RemoveLoweredAttr would leave the raw `[[tapa::...]]` text in the
// generated source for the vendor to reject.
constexpr bool IsTapaAttr(clang::attr::Kind kind) {
  switch (kind) {
    case clang::attr::TapaAggregate:
    case clang::attr::TapaArrayMap:
    case clang::attr::TapaBalance:
    case clang::attr::TapaBindOp:
    case clang::attr::TapaDependence:
    case clang::attr::TapaFlatten:
    case clang::attr::TapaLatency:
    case clang::attr::TapaPartition:
    case clang::attr::TapaPipeline:
    case clang::attr::TapaStorage:
    case clang::attr::TapaTarget:
    case clang::attr::TapaTripcount:
    case clang::attr::TapaUnroll:
      return true;
    default:
      return false;
  }
}

// TapaPipeline is accepted on declarations for legacy compatibility, and
// TapaTarget belongs on functions. Neither is a variable pragma.
constexpr bool IsTapaDeclAttr(clang::attr::Kind kind) {
  return IsTapaAttr(kind) && kind != clang::attr::TapaPipeline &&
         kind != clang::attr::TapaTarget;
}

// Lower a function-level [[tapa::pipeline]] into the function's body.
//
// The attribute is accepted on declarations for legacy compatibility (it is
// how a vendor `#pragma HLS pipeline` written at function scope migrates),
// but nothing lowered it: the pragma was never emitted and the raw
// `[[tapa::pipeline(...)]]` text reached the vendor compiler verbatim.
void LowerFuncAttrPragmas(const clang::FunctionDecl* func,
                          const Backend& backend, EditSink& edits) {
  if (!func->isThisDeclarationADefinition()) return;
  const clang::Stmt* body = func->getBody();
  if (body == nullptr) return;
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() != clang::attr::TapaPipeline) continue;
    const auto* pa = llvm::cast<clang::TapaPipelineAttr>(attr);
    backend.LowerPipeline(pa->getII(), pa->getStyle().str(), body, edits);
  }
}

// Drop a function-level [[tapa::pipeline]] without lowering it. A task that
// is not the current one keeps only its signature, so there is no body to
// carry the pragma — but the attribute text would still reach the vendor.
void RemoveFuncAttrs(const clang::FunctionDecl* func, EditSink& edits) {
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() == clang::attr::TapaPipeline) {
      RemoveLoweredAttr(edits, attr->getRange());
    }
  }
}

void LowerFuncAttrs(const clang::FunctionDecl* func, const Backend& backend,
                    EditSink& edits) {
  LowerFuncAttrPragmas(func, backend, edits);
  RemoveFuncAttrs(func, edits);
}

// Lower every [[tapa::*]] loop and variable attribute in a function body to
// backend pragmas and remove the attribute text. Walks only this body.
void LowerAttrs(const clang::Stmt* stmt, clang::ASTContext& ctx,
                const Backend& backend, EditSink& edits, bool in_block) {
  if (stmt == nullptr) return;
  // A DeclStmt is "in a block" exactly when its parent statement is one;
  // the recursion knows the parent, so no parent-map query is needed.
  const bool children_in_block = llvm::isa<clang::CompoundStmt>(stmt);
  for (const clang::Stmt* child : stmt->children()) {
    LowerAttrs(child, ctx, backend, edits, children_in_block);
  }

  if (const auto* decls = llvm::dyn_cast<clang::DeclStmt>(stmt)) {
    // A declaration not directly inside a block (a for-init, an
    // if/switch init-statement) has nowhere a following pragma can go:
    // after it is INSIDE the for parentheses, which is syntactically
    // invalid. Diagnose rather than emit it.
    // One spelling serves every declarator it announces (int a[4], b[4];
    // attaches to both VarDecls): emit one pragma per variable, but remove
    // each source range exactly once.
    std::set<std::pair<unsigned, unsigned>> lowered;
    const clang::SourceManager& sm = edits.getSourceMgr();
    for (const clang::Decl* decl : decls->decls()) {
      const auto* var = llvm::dyn_cast<clang::VarDecl>(decl);
      if (var == nullptr) continue;
      for (const clang::Attr* attr : var->attrs()) {
        if (!IsTapaDeclAttr(attr->getKind())) continue;
        if (!in_block) {
          ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                           attr->getRange().getBegin(),
                           "[[tapa::%0]] on a declaration outside a block "
                           "(a for-init or condition declaration) has no "
                           "region to lower into; move the declaration into "
                           "a block")
              .AddString(attr->getSpelling());
          continue;
        }
        backend.LowerDeclAttr(*attr, *var, *decls, edits);
        const clang::SourceRange r = attr->getRange();
        if (lowered
                .insert({sm.getFileOffset(r.getBegin()),
                         sm.getFileOffset(r.getEnd())})
                .second) {
          RemoveLoweredAttr(edits, r);
        }
      }
    }
  }

  const auto* attributed = llvm::dyn_cast<clang::AttributedStmt>(stmt);
  if (attributed == nullptr) return;
  // On a loop the pragma lands inside the loop body, and on an `if` or a
  // bare block inside its braces — both are the region the vendor pragma
  // was written in. Anything else (a `return`, a call) takes the pragma
  // directly before it, which is that statement's enclosing region, again
  // matching the vendor spelling. Never drop the attribute.
  const clang::Stmt* sub = attributed->getSubStmt();
  if (const auto* if_stmt = llvm::dyn_cast<clang::IfStmt>(sub)) {
    const bool braced = llvm::isa<clang::CompoundStmt>(if_stmt->getThen());
    if (!braced || if_stmt->getElse() != nullptr) {
      for (const clang::Attr* attr : attributed->getAttrs()) {
        if (!IsTapaAttr(attr->getKind())) continue;
        if (braced) {
          ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                           attr->getRange().getBegin(),
                           "[[tapa::%0]] on an `if` with an `else` would "
                           "constrain only the then-branch; put it on the "
                           "braced block of the branch you mean")
              .AddString(attr->getSpelling());
        } else {
          ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                           attr->getRange().getBegin(),
                           "[[tapa::%0]] on a braceless `if` would "
                           "constrain the enclosing region instead; add "
                           "braces to the branch")
              .AddString(attr->getSpelling());
        }
      }
      return;
    }
  }
  const clang::Stmt* body = GetLoopBody(sub);
  if (body == nullptr) body = GetRegionBody(sub);
  if (body == nullptr) body = sub;
  for (const clang::Attr* attr : attributed->getAttrs()) {
    if (!IsTapaAttr(attr->getKind())) continue;
    switch (attr->getKind()) {
      case clang::attr::TapaPipeline: {
        const auto* pa = llvm::cast<clang::TapaPipelineAttr>(attr);
        backend.LowerPipeline(pa->getII(), pa->getStyle().str(), body, edits);
        break;
      }
      case clang::attr::TapaUnroll:
        backend.LowerUnroll(
            llvm::cast<clang::TapaUnrollAttr>(attr)->getFactor(), body, edits);
        break;
      default:
        backend.LowerStmtAttr(*attr, body, edits);
        break;
    }
    RemoveLoweredAttr(edits, attr->getRange());
  }
}
// Lower [[tapa::*]] variable attributes on function parameters (array
// ports): the pragma goes at the top of the body, exactly where the old
// vendor pragma sat.
void LowerParamAttrPragmas(const clang::FunctionDecl* func,
                           const Backend& backend, EditSink& edits) {
  if (!func->isThisDeclarationADefinition()) return;
  const clang::Stmt* body = func->getBody();
  if (body == nullptr) return;
  for (const clang::ParmVarDecl* param : func->parameters()) {
    for (const clang::Attr* attr : param->attrs()) {
      if (IsTapaDeclAttr(attr->getKind())) {
        backend.LowerParamAttr(*attr, *param, body, edits);
      }
    }
  }
}

void RemoveParamAttrs(const clang::FunctionDecl* func, EditSink& edits) {
  for (const clang::ParmVarDecl* param : func->parameters()) {
    for (const clang::Attr* attr : param->attrs()) {
      if (IsTapaDeclAttr(attr->getKind())) {
        RemoveLoweredAttr(edits, attr->getRange());
      }
    }
  }
}

void LowerParamAttrs(const clang::FunctionDecl* func, const Backend& backend,
                     EditSink& edits) {
  // Only the DEFINITION carries lowerable parameter attributes. A forward
  // declaration's source range would be removed while its rewriting target
  // (the body) does not exist — the rewriter asserts on the inverted range.
  if (!func->isThisDeclarationADefinition()) return;
  LowerParamAttrPragmas(func, backend, edits);
  RemoveParamAttrs(func, edits);
}

// Report any `[[tapa::...]]` text that survived into the emitted source.
//
// Every TAPA attribute is meant to leave the generated code as a backend
// pragma, or not at all. One that reaches the vendor verbatim is a
// directive the user wrote and the toolchain will silently ignore --
// unknown attributes are not an error in C++ -- so the design synthesizes
// with a constraint quietly missing and nothing anywhere says so.
//
// It happens when an attribute lands on a syntactic subject no lowering
// pass visits. A function-level `[[tapa::partition]]` is the worked
// example: Clang drops it (the attribute applies to variables), so it is
// absent from the AST and no removal pass can even see it, while its text
// sits in the buffer the rewriter copies out.
//
// `[[tapa::target]]` is not in that class and is not reported. It picks the
// backend for a task, which discovery reads and acts on before any of this
// runs; it has no pragma form to be lowered to, and reaching the vendor
// changes nothing because the decision it carried has already been made.
// Comments and string literals copied into the emitted buffer may QUOTE an
// attribute spelling (`// migrate to [[tapa::pipeline]]`, a doc string)
// without being one, and preprocessor directives may DEFINE one (a macro
// whose body builds a `[[tapa::...]]` that selective expansion splices
// into user source -- the definition text itself never reaches the vendor
// as an attribute). The scan must not report those. Blanking them (every
// non-newline byte, so line numbers still match) keeps the guard about
// actual attribute text.
std::string BlankCommentsAndStrings(llvm::StringRef code) {
  std::string out = code.str();
  const size_t n = out.size();
  // One preprocessor directive, backslash continuations included.
  const auto directive_end = [&out, n](size_t i) {
    size_t end = out.find('\n', i);
    end = end == std::string::npos ? n : end;
    while (end > 0 && end < n && out[end - 1] == '\\') {
      const size_t next = out.find('\n', end + 1);
      end = next == std::string::npos ? n : next;
    }
    return end;
  };
  for (size_t i = 0; i < n;) {
    size_t end = n;
    size_t first = i;
    while (first < n && (out[first] == ' ' || out[first] == '\t')) ++first;
    if (first < n && out[first] == '#') {
      end = directive_end(first);
    } else if (out[i] == '/' && i + 1 < n && out[i + 1] == '/') {
      end = out.find('\n', i);
      end = end == std::string::npos ? n : end;
    } else if (out[i] == '/' && i + 1 < n && out[i + 1] == '*') {
      end = out.find("*/", i + 2);
      end = end == std::string::npos ? n : end + 2;
    } else if (out[i] == '"' || out[i] == '\'') {
      const char quote = out[i];
      end = i + 1;
      while (end < n && out[end] != quote) {
        end += out[end] == '\\' ? 2 : 1;
      }
      end = end < n ? end + 1 : n;
    } else if (out[i] == 'R' && i + 1 < n && out[i + 1] == '"') {
      // Raw string literal: R"delim( ... )delim".
      const size_t open = out.find('(', i + 2);
      if (open == std::string::npos) {
        ++i;
        continue;
      }
      const std::string close = ")" + out.substr(i + 2, open - (i + 2)) + "\"";
      end = out.find(close, open + 1);
      end = end == std::string::npos ? n : end + close.size();
    } else {
      ++i;
      continue;
    }
    for (size_t j = i; j < end; ++j) {
      if (out[j] != '\n') out[j] = ' ';
    }
    i = end;
  }
  return out;
}

}  // namespace

void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef file,
                       clang::ASTContext& ctx) {
  const std::string scannable = BlankCommentsAndStrings(code);
  code = scannable;
  // The same file renders in every TU that sights it, so the same leak
  // reaches this scan once per TU; report it once per file and spelling.
  static std::set<std::string> reported;
  constexpr llvm::StringLiteral kMarker("[[tapa::");
  for (size_t pos = code.find(kMarker); pos != llvm::StringRef::npos;
       pos = code.find(kMarker, pos + kMarker.size())) {
    const llvm::StringRef rest = code.substr(pos + kMarker.size());
    const size_t end = rest.find_first_not_of(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_");
    const llvm::StringRef name =
        end == llvm::StringRef::npos ? rest : rest.substr(0, end);
    if (name == "target") continue;
    if (!reported.insert((file + ":" + name).str()).second) continue;
    const unsigned line = code.substr(0, pos).count('\n') + 1;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, {},
                     "[[tapa::%0]] was not lowered and would reach the vendor "
                     "verbatim in the mirrored file '%1' (line %2); "
                     "it is on a subject no pragma is emitted for")
        << name << file << line;
  }
}

// The primary-template pattern of a template-specialization task's definition
// (the decl that actually appears in the source), or nullptr.
const clang::FunctionDecl* SpecPrimary(const TaskModel& model) {
  if (!model.is_template_spec || model.def == nullptr) return nullptr;
  if (const clang::FunctionDecl* pattern =
          model.def->getTemplateInstantiationPattern()) {
    return pattern->getCanonicalDecl();
  }
  // A TU that never instantiates the task resolves its model to the primary
  // template itself (see ProgramBuilder::TuView): the pattern is already in
  // hand, and the shared header defining it must take the same guard there
  // as in the instantiating TU.
  return model.def->getDescribedFunctionTemplate() != nullptr
             ? model.def->getCanonicalDecl()
             : nullptr;
}

namespace {

const Backend& BackendFor(SynthTarget target, const Backend& hls,
                          const Backend& vitis, const Backend& ignore) {
  switch (target) {
    case SynthTarget::kXilinxHls:
      return hls;
    case SynthTarget::kXilinxVitis:
      return vitis;
    case SynthTarget::kIgnore:
      return ignore;
  }
  return hls;
}

std::string RewrittenSignature(const clang::FunctionDecl* func,
                               clang::SourceLocation begin, EditSink& edits) {
  clang::SourceLocation end = func->getBody()->getBeginLoc();
  if (end.isMacroID()) {
    // The body is spelled by a macro invocation; the signature text ends
    // where that invocation begins (the splice replaces the rest).
    end = OutermostInvocation(edits.getSourceMgr(), end).getBegin();
  }
  return edits.getRewrittenText(
      clang::CharSourceRange::getCharRange(begin, end));
}

clang::SourceRange DefinitionRange(const clang::FunctionDecl* func,
                                   const EditSink& edits) {
  const clang::SourceManager& sm = edits.getSourceMgr();
  clang::SourceLocation begin = func->getBeginLoc();
  auto take_earlier = [&](clang::SourceLocation candidate) {
    if (candidate.isValid() && sm.isBeforeInTranslationUnit(candidate, begin)) {
      begin = candidate;
    }
  };
  if (const clang::FunctionTemplateDecl* tpl =
          func->getDescribedFunctionTemplate()) {
    take_earlier(tpl->getBeginLoc());
  }
  for (const clang::Attr* attr : func->attrs()) {
    if (!IsTapaAttr(attr->getKind())) continue;
    clang::SourceLocation attr_begin = attr->getRange().getBegin();
    if (attr_begin.isFileID()) {
      const clang::FileID file = sm.getFileID(attr_begin);
      const llvm::StringRef source = sm.getBufferData(file);
      unsigned offset = sm.getFileOffset(attr_begin);
      // Clang's attribute range starts at `tapa`, not the enclosing `[[`.
      // Find that introducer on the same source line so a guard directive
      // never lands in the middle of `[[tapa::...]]`.
      while (offset >= 2 && source[offset - 1] != '\n') {
        if (source.substr(offset - 2, 2) == "[[") {
          attr_begin =
              sm.getLocForStartOfFile(file).getLocWithOffset(offset - 2);
          break;
        }
        --offset;
      }
    }
    take_earlier(attr_begin);
  }
  return {begin, func->getEndLoc()};
}

std::string GuardOpening(const std::vector<std::string>& defines) {
  if (defines.size() == 1) return "#ifdef " + defines.front() + "\n";
  std::string opening = "#if ";
  for (size_t i = 0; i < defines.size(); ++i) {
    if (i > 0) opening += " || ";
    opening += "defined(" + defines[i] + ")";
  }
  return opening + "\n";
}

}  // namespace

void RewriteTreeFiles(const Program& program, SynthTarget default_target,
                      const Backend& hls, const Backend& vitis,
                      const Backend& ignore, clang::ASTContext& ctx,
                      TreeSession& session) {
  EditSink& edits = session.edits();
  const Backend& shared = BackendFor(default_target, hls, vitis, ignore);

  std::map<const clang::FunctionDecl*, std::vector<const TaskModel*>> specs;
  for (const auto& [name, model] : program.tasks) {
    (void)name;
    if (const clang::FunctionDecl* primary = SpecPrimary(model)) {
      specs[primary].push_back(&model);
    }
  }

  auto guard_definition =
      [&](const TaskModel& model, const clang::FunctionDecl* func, bool is_top,
          const Backend& backend, const std::vector<std::string>& defines,
          bool rewrite_signature) {
        edits.Describe("signature of task '" + model.name + "'");
        if (rewrite_signature) backend.RewriteSignature(model, is_top, edits);
        // Signature text is shared by the full definition and stub branches.
        // Remove parameter/function attributes once; their body pragmas stay in
        // the guarded full-definition branch below.
        RemoveParamAttrs(func, edits);
        RemoveFuncAttrs(func, edits);
        const clang::SourceRange definition = DefinitionRange(func, edits);
        const std::string stub =
            RewrittenSignature(func, definition.getBegin(), edits);

        const std::string construct = "definition of task '" + model.name + "'";
        if (!session.InsertGuardOpening(definition, GuardOpening(defines),
                                        construct)) {
          return;
        }
        edits.Describe(construct);
        backend.RewriteTaskFunc(model, is_top, edits);
        LowerParamAttrPragmas(func, backend, edits);
        LowerFuncAttrPragmas(func, backend, edits);
        LowerAttrs(func->getBody(), ctx, backend, edits,
                   /*in_block=*/false);

        const std::string closing = "\n#else\n" + stub + ";\n#endif\n";
        session.InsertGuardClosing(definition, closing, construct);
      };

  // Plain task definitions: unconditional signatures, one guarded body each.
  for (const auto& [name, model] : program.tasks) {
    if (model.is_template_spec) continue;
    const Backend& backend = BackendFor(model.target, hls, vitis, ignore);
    if (!model.def->isThisDeclarationADefinition()) {
      // A TU holding only a declaration of the task (the shared header
      // announcing a task another TU defines) takes the declaration-level
      // rewrites the defining TU applies to that same declaration through
      // its redeclaration chain: the signature rewrite, and the extern "C"
      // wrap of the Vitis top. Guards and bodies exist only at the
      // definition, which this TU cannot see.
      edits.Describe("declaration of task '" + model.name + "'");
      backend.RewriteSignature(model, name == program.top, edits);
      backend.RewriteTaskFunc(model, name == program.top, edits);
      continue;
    }
    guard_definition(model, model.def, name == program.top, backend,
                     {TaskGuardDefine(model.name)},
                     /*rewrite_signature=*/true);
  }

  // Remaining source functions: template-task primaries, task redeclarations,
  // unreachable upper tasks, and helpers, classified per decl and rewritten
  // unconditionally -- the rewrites are identical for every task variant.
  for (const clang::FunctionDecl* func : program.file_funcs) {
    const auto task = program.tasks.find(func->getNameAsString());
    if (task != program.tasks.end() && !task->second.is_template_spec) {
      if (!func->isThisDeclarationADefinition()) RemoveInline(func, edits);
      continue;
    }

    const clang::FunctionDecl* canonical = func->getCanonicalDecl();
    if (const auto spec = specs.find(canonical); spec != specs.end()) {
      const TaskModel& representative = *spec->second.front();
      TaskModel primary = representative;
      primary.def = func;
      std::vector<std::string> defines;
      defines.reserve(spec->second.size());
      for (const TaskModel* model : spec->second) {
        defines.push_back(TaskGuardDefine(model->name));
      }
      guard_definition(primary, func, /*is_top=*/false,
                       BackendFor(representative.target, hls, vitis, ignore),
                       defines, /*rewrite_signature=*/false);
    } else if (GetTapaTaskObject(func->getBody()) != nullptr) {
      TaskModel model;
      model.def = func;
      model.level = TaskLevel::kUpper;
      edits.Describe("unreachable task '" + func->getNameAsString() + "'");
      shared.RewriteSignature(model, /*is_top=*/false, edits);
      RemoveFuncAttrs(func, edits);
      shared.StripOtherTask(func, edits);
    } else {
      edits.Describe("helper function '" + func->getNameAsString() + "'");
      shared.RewriteHelperFunc(func, edits);
      if (func->isThisDeclarationADefinition()) {
        LowerParamAttrs(func, shared, edits);
        LowerFuncAttrs(func, shared, edits);
        LowerAttrs(func->getBody(), ctx, shared, edits,
                   /*in_block=*/false);
      }
    }
  }

  for (const clang::FunctionDecl* func : program.local_funcs) {
    edits.Describe("helper function '" + func->getNameAsString() + "'");
    shared.RewriteHelperFunc(func, edits);
    LowerParamAttrs(func, shared, edits);
    LowerFuncAttrs(func, shared, edits);
    LowerAttrs(func->getBody(), ctx, shared, edits,
               /*in_block=*/false);
  }

  // Insert wrappers only after every definition guard is in place. A wrapper
  // shares its invoker's end offset; doing this last guarantees it lands after
  // that invoker's #endif, under the specialization's own guard alone.
  for (const auto& [name, model] : program.tasks) {
    if (!model.is_template_spec || model.invoker == nullptr) continue;
    const std::string define = TaskGuardDefine(model.name);
    const Backend& backend = BackendFor(model.target, hls, vitis, ignore);
    edits.Describe("wrapper for template task '" + name + "'");
    edits.InsertTextAfterToken(model.invoker->getEndLoc(),
                               "\n#ifdef " + define + "\n" +
                                   GenerateWrapper(model, backend, ctx) +
                                   "#endif\n");
  }
}

}  // namespace tapa::cc
