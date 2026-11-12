#include "rewrite.h"

#include <cctype>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <utility>
#include <vector>

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
// `TapaAggregate..TapaUnroll` covers today's fifteen only by accident of
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
    case clang::attr::TapaFrame:
    case clang::attr::TapaLatency:
    case clang::attr::TapaPartition:
    case clang::attr::TapaPipeline:
    case clang::attr::TapaRing:
    case clang::attr::TapaStorage:
    case clang::attr::TapaTarget:
    case clang::attr::TapaTripcount:
    case clang::attr::TapaUnroll:
      return true;
    default:
      return false;
  }
}

// TapaPipeline belongs on functions and statements, TapaTarget on functions.
// Neither is a variable pragma.
constexpr bool IsTapaDeclAttr(clang::attr::Kind kind) {
  return IsTapaAttr(kind) && kind != clang::attr::TapaPipeline &&
         kind != clang::attr::TapaTarget;
}

// Lower a function-level [[tapa::pipeline]] into the function's body.
void LowerFuncAttrPragmas(const clang::FunctionDecl* func,
                          const Backend& backend, EditSink& edits) {
  if (!func->isThisDeclarationADefinition()) return;
  const clang::Stmt* body = func->getBody();
  if (body == nullptr) return;
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() != clang::attr::TapaPipeline) continue;
    const auto* pa = llvm::cast<clang::TapaPipelineAttr>(attr);
    backend.LowerFuncPipeline(pa->getII(), pa->getStyle().str(), body, edits);
  }
}

// Drop the function-level attribute text once it has been consumed: a
// [[tapa::pipeline]] is lowered into the body (a task's stub branch keeps
// only its signature, so no body carries the pragma there), and a
// [[tapa::target]] was read by discovery. Neither may reach the vendor; a
// target left in front of a Vitis top would precede its `extern "C"`.
void RemoveFuncAttrs(const clang::FunctionDecl* func, EditSink& edits) {
  for (const clang::Attr* attr : func->attrs()) {
    if (attr->getKind() == clang::attr::TapaPipeline ||
        attr->getKind() == clang::attr::TapaTarget) {
      RemoveLoweredAttr(edits, attr->getRange());
    }
  }
}

// On a definition: lower the pragma, then drop the attribute text. On a
// declaration: there is no body to lower into and the text would reach the
// vendor verbatim, so say where the attribute belongs instead.
void LowerFuncAttrs(const clang::FunctionDecl* func, const Backend& backend,
                    EditSink& edits) {
  if (!func->isThisDeclarationADefinition()) {
    for (const clang::Attr* attr : func->attrs()) {
      // Discovery reads a target through every declaration; consumed, it
      // must not precede the `extern "C"` a Vitis top's declaration gets.
      if (attr->getKind() == clang::attr::TapaTarget) {
        RemoveLoweredAttr(edits, attr->getRange());
      }
      if (attr->getKind() != clang::attr::TapaPipeline) continue;
      ReportCustomDiag(func->getASTContext(), clang::DiagnosticsEngine::Error,
                       attr->getRange().getBegin(),
                       "[[tapa::%0]] on a function declaration: the pragma "
                       "lowers into the body, so put it on the definition")
          .AddString(attr->getSpelling());
      // The error already fails the run; dropping the text keeps the
      // leaked-attribute scan from echoing the same site.
      RemoveLoweredAttr(edits, attr->getRange());
    }
    return;
  }
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
  const clang::Stmt* body = LoopBody(sub);
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
// ports): the pragma goes at the top of the body.
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
  if (!func->isThisDeclarationADefinition()) {
    // There is no body to lower into, but declaration text must not leak to
    // the vendor. The definition owns the pragma's semantics.
    RemoveParamAttrs(func, edits);
    return;
  }
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
// Whether the quote at `i` separates digits (`1'000`, C++14) rather than
// opening a character literal: the token it is in starts as a number, not
// as an encoding prefix such as `u8`.
bool IsDigitSeparator(const std::string& code, size_t i) {
  size_t begin = i;
  while (begin > 0 &&
         (std::isalnum(static_cast<unsigned char>(code[begin - 1])) != 0 ||
          code[begin - 1] == '_' || code[begin - 1] == '.' ||
          code[begin - 1] == '\'')) {
    --begin;
  }
  const auto digit = [&](size_t at) {
    return at < i && std::isdigit(static_cast<unsigned char>(code[at])) != 0;
  };
  return digit(begin) || (code[begin] == '.' && digit(begin + 1));
}

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
    } else if (out[i] == '\'' && IsDigitSeparator(out, i)) {
      ++i;
      continue;
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

void StripSkippedAttrs(const std::vector<clang::SourceRange>& skipped,
                       const std::set<clang::FileID>& mirrored,
                       clang::ASTContext& ctx, EditSink& edits) {
  const clang::SourceManager& sm = ctx.getSourceManager();
  for (const clang::SourceRange& range : skipped) {
    const clang::SourceLocation begin = range.getBegin();
    if (!begin.isFileID() || mirrored.count(sm.getFileID(begin)) == 0) {
      continue;
    }
    bool invalid = false;
    const char* data = sm.getCharacterData(begin, &invalid);
    if (invalid) continue;
    const std::string text = BlankCommentsAndStrings(llvm::StringRef(
        data, sm.getFileOffset(range.getEnd()) - sm.getFileOffset(begin)));
    for (size_t pos = text.find("[["); pos != std::string::npos;
         pos = text.find("[[", pos + 2)) {
      const size_t name = text.find_first_not_of(" \t\n", pos + 2);
      if (name == std::string::npos || text.compare(name, 6, "tapa::") != 0) {
        continue;
      }
      // The group runs to the `]]` that closes it.
      int depth = 0;
      size_t end = pos;
      for (; end < text.size(); ++end) {
        if (text[end] == '[' || text[end] == '(') ++depth;
        if ((text[end] == ']' || text[end] == ')') && --depth == 0) break;
      }
      if (end == text.size()) break;  // unclosed: the leak scan reports it
      const llvm::StringRef spelling = llvm::StringRef(text).substr(name + 6);
      const clang::SourceLocation at = begin.getLocWithOffset(pos);
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Remark, at,
                       "[[tapa::%0]] is in code the preprocessor skips and "
                       "takes no effect")
          << spelling.take_while(
                 [](char c) { return std::isalnum(c) != 0 || c == '_'; });
      edits.RemoveText(at, static_cast<unsigned>(end + 1 - pos));
      pos = end;
    }
  }
}

void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef file,
                       clang::ASTContext& ctx,
                       std::set<std::string>& reported) {
  const std::string scannable = BlankCommentsAndStrings(code);
  code = scannable;
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

namespace {

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

const Backend& BackendFor(SynthTarget target, const Backends& backends) {
  switch (target) {
    case SynthTarget::kXilinxHls:
      return backends.hls;
    case SynthTarget::kXilinxVitis:
    case SynthTarget::kXilinxSlash:
      return backends.vitis;
    case SynthTarget::kIgnore:
      return backends.ignore;
    case SynthTarget::kAie:
      return backends.aie;
  }
  return backends.hls;
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
  // A macro spelling only a prefix of the definition (a specifier, an
  // attribute: `KERNEL void Leaf(...) {...}`) leaves the rest spelled in
  // the file, so the definition starts at that invocation. Only one
  // invocation spelling the whole definition owns it.
  const clang::SourceLocation end = func->getEndLoc();
  if (begin.isMacroID()) {
    const clang::CharSourceRange invocation = OutermostInvocation(sm, begin);
    if (end.isFileID() ||
        OutermostInvocation(sm, end).getBegin() != invocation.getBegin()) {
      begin = invocation.getBegin();
    }
  }
  return {begin, end};
}

// `defined(A) || defined(B) || ...` — one guard condition over a define set.
std::string DefinedAny(const std::vector<std::string>& defines) {
  std::string condition;
  for (size_t i = 0; i < defines.size(); ++i) {
    if (i > 0) condition += " || ";
    condition += "defined(" + defines[i] + ")";
  }
  return condition;
}

std::string GuardOpening(const std::vector<std::string>& defines) {
  if (defines.size() == 1) return "#ifdef " + defines.front() + "\n";
  return "#if " + DefinedAny(defines) + "\n";
}

std::string GuardElif(const std::vector<std::string>& defines) {
  return "#elif " + DefinedAny(defines) + "\n";
}

// The `#else` branch closing a guarded definition: the signature-only stub
// declaration, then `#endif`.
std::string GuardStubClosing(const std::string& stub) {
  return "\n#else\n" + stub + ";\n#endif\n";
}

}  // namespace

void RewriteTreeFiles(const Program& program, SynthTarget default_target,
                      const Backends& backends, clang::ASTContext& ctx,
                      TreeSession& session) {
  EditSink& edits = session.edits();
  const Backend& shared = BackendFor(default_target, backends);

  std::map<const clang::FunctionDecl*, std::vector<const TaskModel*>> specs;
  for (const auto& [name, model] : program.tasks) {
    (void)name;
    if (const clang::FunctionDecl* primary = SpecPrimary(model)) {
      specs[primary].push_back(&model);
    }
  }

  // The stub branch is the spelled signature text (RewrittenSignature). A
  // definition whose signature is macro-owned -- the whole definition spelled
  // by one invocation, not merely its body, which the splice machinery
  // rewrites through -- has no spelled text: the stub would collapse to a
  // bare `;` and lose the declaration. Reject the shape before emitting
  // anything.
  auto reject_macro_owned_definition = [&](const clang::SourceRange& definition,
                                           const std::string& task_name) {
    if (!definition.getBegin().isMacroID()) return false;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                     definition.getBegin(),
                     "task '%0' is defined by a macro invocation: its "
                     "signature has no spelled source text for the "
                     "guard's signature-only stub (a macro may spell "
                     "the body, not the whole definition); spell the "
                     "definition outside the macro")
        .AddString(task_name);
    return true;
  };

  // The shared prelude of both guard shapes: the definition range, the
  // macro-owned-signature rejection, the caller's signature-phase edits, and
  // the attribute removal plus stub capture (the signature text is shared by
  // the full-definition and stub branches, so attributes come off once here;
  // their body pragmas stay in the full-definition branch). Nullopt when the
  // definition shape is rejected.
  auto begin_guarded = [&](const clang::FunctionDecl* func,
                           const std::string& task_name,
                           const auto& signature_edits)
      -> std::optional<std::pair<clang::SourceRange, std::string>> {
    const clang::SourceRange definition = DefinitionRange(func, edits);
    if (reject_macro_owned_definition(definition, task_name)) {
      return std::nullopt;
    }
    signature_edits();
    RemoveParamAttrs(func, edits);
    RemoveFuncAttrs(func, edits);
    return {
        {definition, RewrittenSignature(func, definition.getBegin(), edits)}};
  };

  auto guard_definition = [&](const TaskModel& model,
                              const clang::FunctionDecl* func, bool is_top,
                              const Backend& backend,
                              const std::vector<std::string>& defines,
                              bool rewrite_signature) {
    const auto prelude = begin_guarded(func, model.name, [&] {
      edits.Describe("signature of task '" + model.name + "'");
      if (rewrite_signature) backend.RewriteSignature(model, is_top, edits);
    });
    if (!prelude) return;
    const auto& [definition, stub] = *prelude;

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

    session.InsertGuardClosing(definition, GuardStubClosing(stub), construct);
  };

  using MixedVariant = std::pair<TaskModel, std::vector<std::string>>;
  auto guard_mixed_definition = [&](const std::vector<MixedVariant>& variants,
                                    const clang::FunctionDecl* func,
                                    const auto& rewrite_body) {
    const clang::SourceRange definition = DefinitionRange(func, edits);
    if (reject_macro_owned_definition(definition,
                                      variants.front().first.name)) {
      return;
    }
    // Each variant, and the stub, is rewritten on its own copy of the
    // definition, which then replaces it whole: an edit of the main sink
    // inside it (a removed attribute) would shift the replaced range.
    const auto scratch = [&](clang::Rewriter& rewriter) {
      EditSink sink(ctx, rewriter);
      sink.TrackFile(edits.getSourceMgr().getFileID(definition.getBegin()),
                     [](clang::SourceLocation, unsigned, llvm::StringRef) {
                       return true;
                     });
      return sink;
    };
    clang::Rewriter stub_rewriter(edits.getSourceMgr(), edits.getLangOpts());
    EditSink stub_edits = scratch(stub_rewriter);
    RemoveParamAttrs(func, stub_edits);
    RemoveFuncAttrs(func, stub_edits);
    const std::string stub =
        RewrittenSignature(func, definition.getBegin(), stub_edits);
    std::string guarded;
    for (size_t i = 0; i < variants.size(); ++i) {
      auto [model, defines] = variants[i];
      model.def = func;
      guarded += i == 0 ? GuardOpening(defines) : GuardElif(defines);
      const Backend& backend = BackendFor(model.target, backends);
      clang::Rewriter variant_rewriter(edits.getSourceMgr(),
                                       edits.getLangOpts());
      EditSink variant_edits = scratch(variant_rewriter);
      rewrite_body(model, backend, variant_edits);
      guarded += variant_edits.getRewrittenText(definition);
      guarded += "\n";
    }
    guarded += GuardStubClosing(stub);
    if (TreeFileBuffer* buffer =
            session.BufferForLocation(definition.getBegin())) {
      (void)buffer->ReplaceWithLineResnap(
          clang::CharSourceRange::getTokenRange(definition), guarded);
    }
  };

  std::map<SynthTarget, std::vector<std::string>> target_defines;
  for (const auto& [name, model] : program.tasks) {
    (void)name;
    target_defines[model.target].push_back(TaskGuardDefine(model.name));
  }

  // A helper shared by several targets gets one guarded definition variant
  // per target.
  auto guard_helper_definition = [&](const clang::FunctionDecl* func) {
    std::vector<MixedVariant> variants;
    for (const auto& [target, defines] : target_defines) {
      TaskModel model;
      model.name = func->getNameAsString();
      model.def = func;
      model.target = target;
      variants.emplace_back(std::move(model), defines);
    }
    const auto rewrite_helper = [func](TaskModel&, const Backend& backend,
                                       EditSink& variant_edits) {
      backend.RewriteHelperFunc(func, variant_edits);
      LowerFuncAttrs(func, backend, variant_edits);
      LowerParamAttrs(func, backend, variant_edits);
      LowerAttrs(func->getBody(), variant_edits.getASTContext(), backend,
                 variant_edits, /*in_block=*/false);
    };
    guard_mixed_definition(variants, func, rewrite_helper);
  };

  // Plain task definitions: unconditional signatures, one guarded body each.
  for (const auto& [name, model] : program.tasks) {
    if (model.is_template_spec) continue;
    const Backend& backend = BackendFor(model.target, backends);
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
      LowerFuncAttrs(model.def, backend, edits);
      LowerParamAttrs(model.def, backend, edits);
      continue;
    }
    if (name == program.top && model.target == SynthTarget::kAie &&
        !model.rings.empty()) {
      // An AI Engine top whose frame arguments have rings: its graph's
      // definition, and the PL kernel carrying those arguments where only
      // PL drives the graph (see PlKernelGuardDefine); which one a platform
      // builds is synth's choice.
      TaskModel pl = model;
      pl.target = default_target;
      const auto rewrite_top = [](TaskModel& variant, const Backend& backend,
                                  EditSink& variant_edits) {
        RemoveParamAttrs(variant.def, variant_edits);
        RemoveFuncAttrs(variant.def, variant_edits);
        if (variant.target != SynthTarget::kAie) {
          backend.RewritePlKernel(variant, variant_edits);
          return;
        }
        backend.RewriteTaskFunc(variant, /*is_top=*/true, variant_edits);
        LowerParamAttrPragmas(variant.def, backend, variant_edits);
        LowerFuncAttrPragmas(variant.def, backend, variant_edits);
        LowerAttrs(variant.def->getBody(), variant_edits.getASTContext(),
                   backend, variant_edits, /*in_block=*/false);
      };
      guard_mixed_definition({{model, {TaskGuardDefine(model.name)}},
                              {pl, {PlKernelGuardDefine(model.name)}}},
                             model.def, rewrite_top);
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
      if (!func->isThisDeclarationADefinition()) {
        RemoveInline(func, edits);
        LowerFuncAttrs(func, shared, edits);
        LowerParamAttrs(func, shared, edits);
      }
      continue;
    }

    const clang::FunctionDecl* canonical = func->getCanonicalDecl();
    if (const auto spec = specs.find(canonical); spec != specs.end()) {
      std::map<SynthTarget, std::vector<const TaskModel*>> by_target;
      for (const TaskModel* model : spec->second) {
        by_target[model->target].push_back(model);
      }
      const auto rewrite_task = [](TaskModel& model, const Backend& backend,
                                   EditSink& variant_edits) {
        backend.RewriteTaskFunc(model, /*is_top=*/false, variant_edits);
        LowerParamAttrPragmas(model.def, backend, variant_edits);
        LowerFuncAttrPragmas(model.def, backend, variant_edits);
        LowerAttrs(model.def->getBody(), variant_edits.getASTContext(), backend,
                   variant_edits, /*in_block=*/false);
      };
      if (by_target.size() == 1) {
        const TaskModel& representative = *spec->second.front();
        TaskModel primary = representative;
        primary.def = func;
        std::vector<std::string> defines;
        defines.reserve(spec->second.size());
        for (const TaskModel* model : spec->second) {
          defines.push_back(TaskGuardDefine(model->name));
        }
        guard_definition(primary, func, /*is_top=*/false,
                         BackendFor(representative.target, backends), defines,
                         /*rewrite_signature=*/false);
      } else {
        std::vector<MixedVariant> variants;
        for (const auto& [target, models] : by_target) {
          std::vector<std::string> defines;
          for (const TaskModel* model : models) {
            defines.push_back(TaskGuardDefine(model->name));
          }
          variants.emplace_back(*models.front(), std::move(defines));
        }
        guard_mixed_definition(variants, func, rewrite_task);
      }
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
      if (target_defines.size() > 1 && func->isThisDeclarationADefinition()) {
        guard_helper_definition(func);
      } else {
        shared.RewriteHelperFunc(func, edits);
        LowerFuncAttrs(func, shared, edits);
        if (func->isThisDeclarationADefinition()) {
          LowerParamAttrs(func, shared, edits);
          LowerAttrs(func->getBody(), ctx, shared, edits,
                     /*in_block=*/false);
        }
      }
    }
  }

  for (const clang::FunctionDecl* func : program.local_funcs) {
    edits.Describe("helper function '" + func->getNameAsString() + "'");
    if (target_defines.size() > 1) {
      guard_helper_definition(func);
    } else {
      shared.RewriteHelperFunc(func, edits);
      LowerParamAttrs(func, shared, edits);
      LowerFuncAttrs(func, shared, edits);
      LowerAttrs(func->getBody(), ctx, shared, edits,
                 /*in_block=*/false);
    }
  }

  // Insert wrappers only after every definition guard is in place. A wrapper
  // shares its invoker's end offset; doing this last guarantees it lands after
  // that invoker's #endif, under the specialization's own guard alone.
  for (const auto& [name, model] : program.tasks) {
    if (!model.is_template_spec || model.invoker == nullptr) continue;
    const std::string define = TaskGuardDefine(model.name);
    const Backend& backend = BackendFor(model.target, backends);
    edits.Describe("wrapper for template task '" + name + "'");
    edits.InsertTextAfterToken(model.invoker->getEndLoc(),
                               "\n#ifdef " + define + "\n" +
                                   GenerateWrapper(model, backend, ctx) +
                                   "#endif\n");
  }
  // Each mover follows the AI Engine task it serves, under its own guard.
  for (const Mover& mover : program.movers) {
    edits.Describe("mover '" + mover.name + "'");
    edits.InsertTextAfterToken(mover.after->getEndLoc(),
                               "\n#ifdef " + TaskGuardDefine(mover.name) +
                                   "\n" + shared.GenerateMover(mover) +
                                   "#endif\n");
  }
}

}  // namespace tapa::cc
