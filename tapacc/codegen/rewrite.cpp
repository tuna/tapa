#include "rewrite.h"

#include <set>
#include <string>

#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/StmtCXX.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Rewrite/Core/Rewriter.h"
#include "llvm/ADT/StringRef.h"

#include "emit.h"
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
void LowerFuncAttrs(const clang::FunctionDecl* func, const Backend& backend,
                    clang::Rewriter& rewriter) {
  if (!func->isThisDeclarationADefinition()) return;
  const clang::Stmt* body = func->getBody();
  if (body == nullptr) return;
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() != clang::attr::TapaPipeline) continue;
    const auto* pa = llvm::cast<clang::TapaPipelineAttr>(attr);
    backend.LowerPipeline(pa->getII(), pa->getStyle().str(), body, rewriter);
    RemoveLoweredAttr(rewriter, attr->getRange());
  }
}

// Drop a function-level [[tapa::pipeline]] without lowering it. A task that
// is not the current one keeps only its signature, so there is no body to
// carry the pragma — but the attribute text would still reach the vendor.
void RemoveFuncAttrs(const clang::FunctionDecl* func,
                     clang::Rewriter& rewriter) {
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() == clang::attr::TapaPipeline) {
      RemoveLoweredAttr(rewriter, attr->getRange());
    }
  }
}

// Lower every [[tapa::*]] loop and variable attribute in a function body to
// backend pragmas and remove the attribute text. Walks only this body.
void LowerAttrs(const clang::Stmt* stmt, clang::ASTContext& ctx,
                const Backend& backend, clang::Rewriter& rewriter,
                bool in_block) {
  if (stmt == nullptr) return;
  // A DeclStmt is "in a block" exactly when its parent statement is one;
  // the recursion knows the parent, so no parent-map query is needed.
  const bool children_in_block = llvm::isa<clang::CompoundStmt>(stmt);
  for (const clang::Stmt* child : stmt->children()) {
    LowerAttrs(child, ctx, backend, rewriter, children_in_block);
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
    const clang::SourceManager& sm = rewriter.getSourceMgr();
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
        backend.LowerDeclAttr(*attr, *var, *decls, rewriter);
        const clang::SourceRange r = attr->getRange();
        if (lowered
                .insert({sm.getFileOffset(r.getBegin()),
                         sm.getFileOffset(r.getEnd())})
                .second) {
          RemoveLoweredAttr(rewriter, r);
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
        backend.LowerPipeline(pa->getII(), pa->getStyle().str(), body,
                              rewriter);
        break;
      }
      case clang::attr::TapaUnroll:
        backend.LowerUnroll(
            llvm::cast<clang::TapaUnrollAttr>(attr)->getFactor(), body,
            rewriter);
        break;
      default:
        backend.LowerStmtAttr(*attr, body, rewriter);
        break;
    }
    RemoveLoweredAttr(rewriter, attr->getRange());
  }
}
// Lower [[tapa::*]] variable attributes on function parameters (array
// ports): the pragma goes at the top of the body, exactly where the old
// vendor pragma sat.
void LowerParamAttrs(const clang::FunctionDecl* func, const Backend& backend,
                     clang::Rewriter& rewriter) {
  // Only the DEFINITION carries lowerable parameter attributes. A forward
  // declaration's source range would be removed while its rewriting target
  // (the body) does not exist — the rewriter asserts on the inverted range.
  if (!func->isThisDeclarationADefinition()) return;
  const clang::Stmt* body = func->getBody();
  if (body == nullptr) return;
  for (const clang::ParmVarDecl* param : func->parameters()) {
    for (const clang::Attr* attr : param->attrs()) {
      if (!IsTapaDeclAttr(attr->getKind())) continue;
      backend.LowerParamAttr(*attr, *param, body, rewriter);
      RemoveLoweredAttr(rewriter, attr->getRange());
    }
  }
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
// without being one; the scan must not report those. Blanking them (every
// non-newline byte, so line numbers still match) keeps the guard about
// actual attribute text.
std::string BlankCommentsAndStrings(llvm::StringRef code) {
  std::string out = code.str();
  const size_t n = out.size();
  for (size_t i = 0; i < n;) {
    size_t end = n;
    if (out[i] == '/' && i + 1 < n && out[i + 1] == '/') {
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

void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef task,
                       clang::ASTContext& ctx) {
  const std::string scannable = BlankCommentsAndStrings(code);
  code = scannable;
  // Every emitted file contains all helpers, so one leaked attribute in a
  // shared helper would error identically in every task's file; report
  // once per unique spelling per process.
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
    if (!reported.insert(name.str()).second) continue;
    const unsigned line = code.substr(0, pos).count('\n') + 1;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, {},
                     "[[tapa::%0]] was not lowered and would reach the vendor "
                     "verbatim in the generated code for task '%1' (line %2); "
                     "it is on a subject no pragma is emitted for")
        << name << task << line;
  }
}

}  // namespace

// The primary-template pattern of a template-specialization task's definition
// (the decl that actually appears in the source), or nullptr.
const clang::FunctionDecl* SpecPrimary(const TaskModel& model) {
  if (!model.is_template_spec || model.def == nullptr) return nullptr;
  const clang::FunctionDecl* pattern =
      model.def->getTemplateInstantiationPattern();
  return pattern != nullptr ? pattern->getCanonicalDecl() : nullptr;
}

std::string EmitTaskCode(const Program& program, const TaskModel& task,
                         const Backend& backend, clang::ASTContext& ctx) {
  clang::Rewriter rewriter(ctx.getSourceManager(), ctx.getLangOpts());

  // The source decls that are template patterns for some task specialization,
  // and the pattern for the current task (if it is a specialization). These
  // primaries appear as functions in the file but are keyed in program.tasks by
  // mangled name, so they are handled here rather than as helpers.
  std::set<const clang::FunctionDecl*> spec_primaries;
  for (const auto& [name, model] : program.tasks) {
    if (const clang::FunctionDecl* primary = SpecPrimary(model)) {
      spec_primaries.insert(primary);
    }
  }
  const clang::FunctionDecl* current_primary = SpecPrimary(task);

  // Non-template task functions: signature rewritten per level; current task
  // keeps a rewritten body, the rest become signatures.
  for (const auto& [name, model] : program.tasks) {
    if (model.is_template_spec) continue;  // reached via its primary below
    const bool is_top = name == program.top;
    backend.RewriteSignature(model, is_top, rewriter);
    if (name == task.name) {
      backend.RewriteTaskFunc(model, is_top, rewriter);
      LowerParamAttrs(model.def, backend, rewriter);
      LowerFuncAttrs(model.def, backend, rewriter);
      LowerAttrs(model.def->getBody(), ctx, backend, rewriter,
                 /*in_block=*/false);
    } else {
      RemoveFuncAttrs(model.def, rewriter);
      backend.StripOtherTask(model.def, rewriter);
    }
  }

  // Remaining source functions: template-task primaries and plain helpers.
  for (const clang::FunctionDecl* func : program.file_funcs) {
    if (program.tasks.count(func->getNameAsString()) != 0 &&
        !program.tasks.at(func->getNameAsString()).is_template_spec) {
      // A redeclaration of an already-handled task: Vitis rejects `inline`
      // on tasks, so strip it here too (the definition was handled above).
      if (!func->isThisDeclarationADefinition()) {
        RemoveInline(func, rewriter);
      }
      continue;
    }
    const clang::FunctionDecl* canonical = func->getCanonicalDecl();
    if (spec_primaries.count(canonical) != 0) {
      // A template primary whose specialization(s) are tasks.
      if (current_primary != nullptr && canonical == current_primary) {
        // Rewrite the primary template using its OWN parameters: a bare
        // template-parameter type (e.g. `tapa_mmap_type mmap`) is classified as
        // a scalar here (no interface pragma) -- only the concrete wrapper
        // does.
        TaskModel primary = task;
        primary.def = func;
        backend.RewriteTaskFunc(primary, /*is_top=*/false, rewriter);
        LowerParamAttrs(func, backend, rewriter);
        LowerFuncAttrs(func, backend, rewriter);
        LowerAttrs(func->getBody(), ctx, backend, rewriter,
                   /*in_block=*/false);
      } else {
        RemoveFuncAttrs(func, rewriter);
        backend.StripOtherTask(func, rewriter);
      }
    } else if (GetTapaTaskObject(func->getBody()) != nullptr) {
      // Unreachable upper-level task: discovery is reachability-based, so it
      // is not in program.tasks, but its body still invokes sub-tasks and
      // would no longer type-check against their rewritten signatures. Strip
      // it like any non-current task.
      TaskModel model;
      model.def = func;
      model.level = TaskLevel::kUpper;
      backend.RewriteSignature(model, /*is_top=*/false, rewriter);
      RemoveFuncAttrs(func, rewriter);
      backend.StripOtherTask(func, rewriter);
    } else {
      backend.RewriteHelperFunc(func, rewriter);
      // A redeclaration's getBody() recovers the DEFINITION's body across
      // the chain: lowering here would lower the same attributes a second
      // time and the second RemoveLoweredAttr would probe an already-removed
      // range. Only definitions carry lowerable attributes.
      if (func->isThisDeclarationADefinition()) {
        LowerParamAttrs(func, backend, rewriter);
        LowerFuncAttrs(func, backend, rewriter);
        LowerAttrs(func->getBody(), ctx, backend, rewriter,
                   /*in_block=*/false);
      }
    }
  }

  // Internal-linkage helpers and methods take the same inline policy as any
  // other helper: without it they carry no inlining control and the vendor
  // decides the hierarchy, which is what the keyword rule exists to prevent.
  //
  // No name-based exclusion here. Tasks are discovered from `file_funcs`,
  // which holds only global definitions, so nothing in `local_funcs` can be
  // a task — a name test could only strike a helper that happens to share a
  // name with one, denying it the policy for no reason.
  for (const clang::FunctionDecl* func : program.local_funcs) {
    if (GetTapaTaskObject(func->getBody()) != nullptr) {
      // Task-shaped, but internal linkage puts it beyond discovery's reach,
      // so it can never be invoked as a task. Say so rather than silently
      // leaving it alone: its body still invokes sub-tasks whose signatures
      // this pass rewrites, so the result will not compile, and the cause is
      // a long way from the error.
      llvm::errs() << "Warning: " << func->getNameAsString()
                   << " builds a tapa::task() but has internal linkage, so it"
                      " cannot be discovered as a task; give it external"
                      " linkage\n";
      continue;
    }
    backend.RewriteHelperFunc(func, rewriter);
    LowerParamAttrs(func, backend, rewriter);
    LowerFuncAttrs(func, backend, rewriter);
    LowerAttrs(func->getBody(), ctx, backend, rewriter,
               /*in_block=*/false);
  }

  // Emit the mangled wrapper for the current specialization after its invoker.
  if (task.is_template_spec) {
    InsertWrapper(task, backend, ctx, rewriter);
  }

  const clang::SourceManager& sm = ctx.getSourceManager();
  const clang::FileID main_file = sm.getMainFileID();
  const llvm::RewriteBuffer* buffer = rewriter.getRewriteBufferFor(main_file);
  const std::string code = buffer == nullptr
                               ? sm.getBufferData(main_file).str()  // no edits
                               : std::string(buffer->begin(), buffer->end());
  ReportLeakedAttrs(code, task.name, ctx);
  return code;
}

}  // namespace tapa::cc
