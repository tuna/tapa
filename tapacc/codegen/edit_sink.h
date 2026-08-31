#ifndef TAPA_CODEGEN_EDIT_SINK_H_
#define TAPA_CODEGEN_EDIT_SINK_H_

#include <functional>
#include <map>
#include <string>
#include <utility>

#include "clang/AST/ASTContext.h"
#include "clang/Basic/LangOptions.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Rewrite/Core/Rewriter.h"
#include "llvm/ADT/StringRef.h"

#include "macro_splice.h"

namespace tapa::cc {

// The edit surface every decl-rewrite rule writes through: the exact
// clang::Rewriter calls the rules make, behind one type. The caller
// registers one tracker per mirrored file, so every edit gets `#line`
// re-snap bookkeeping, while a rewrite outside the mirror is a hard
// diagnostic. A macro-owned edit never reaches the Rewriter: it composes
// into token slots on the outermost spelled invocation owning it
// (MacroSplices) and the invocation is spliced once at session end.
class EditSink {
 public:
  using Resnap =
      std::function<bool(clang::SourceLocation, unsigned, llvm::StringRef)>;

  // Rejected edits report through `ctx`; call TrackFile for every mirrored
  // FileID before applying rewrite rules.
  EditSink(clang::ASTContext& ctx, clang::Rewriter& rewriter);

  void TrackFile(clang::FileID file, Resnap resnap);
  // Installs the splice queue for macro-owned edits (requires a recorded
  // token stream).
  void RouteMacroEdits(MacroSplices* splices) { splices_ = splices; }
  void Describe(std::string construct) { construct_ = std::move(construct); }
  bool CanRewrite(clang::SourceRange range);
  bool CanRewrite(clang::CharSourceRange range);

  bool ReplaceText(clang::SourceRange range, llvm::StringRef text);
  bool ReplaceText(clang::SourceLocation start, unsigned length,
                   llvm::StringRef text);
  bool InsertTextBefore(clang::SourceLocation loc, llvm::StringRef text);
  bool InsertTextAfter(clang::SourceLocation loc, llvm::StringRef text);
  bool InsertTextAfterToken(clang::SourceLocation loc, llvm::StringRef text);
  bool RemoveText(clang::SourceLocation start, unsigned length);
  bool RemoveText(clang::SourceRange range);

  // The splice owning `loc` (a macro location), for rewrite rules that work
  // on the recorded tokens themselves (macro-owned attribute removal).
  // Nullptr after reporting why the edit has no home.
  ExpansionSplice* MacroSpliceFor(clang::SourceLocation loc);
  // Drops tokens [first, last] of `splice` (from MacroSpliceFor),
  // substituting `text`; reports and returns true (rejected) on failure.
  bool DropSpliceTokens(ExpansionSplice* splice, size_t first, size_t last,
                        llvm::StringRef text);

  std::string getRewrittenText(clang::SourceRange range) const;
  std::string getRewrittenText(clang::CharSourceRange range) const;
  clang::SourceManager& getSourceMgr() const;
  const clang::LangOptions& getLangOpts() const;

 private:
  bool CanEdit(clang::SourceLocation begin, clang::SourceLocation end);
  // A file location names a mirrored file (reports once when it does not).
  bool Mirrored(clang::SourceLocation loc);
  // The splice a macro-owned location composes into; nullptr after
  // reporting the precise reason.
  ExpansionSplice* MacroSide(clang::SourceLocation loc);
  // The one splice an edit over [begin, end] composes into: the whole
  // range in one invocation, spelled in a mirrored file.
  ExpansionSplice* MacroHome(clang::SourceLocation begin,
                             clang::SourceLocation end);
  void ReportSpliceReject(clang::SourceLocation loc, llvm::StringRef why);
  // Routes one macro-owned edit through MacroHome; reports and returns
  // true (rejected) when the splice cannot compose it.
  template <typename F>
  bool ViaSplice(clang::SourceLocation begin, clang::SourceLocation end,
                 F&& apply);
  bool Finish(clang::SourceLocation begin, unsigned length,
              llvm::StringRef text, bool rejected);
  unsigned OriginalLength(clang::CharSourceRange range) const;

  clang::ASTContext* ctx_ = nullptr;
  clang::Rewriter& rewriter_;
  MacroSplices* splices_ = nullptr;
  std::map<clang::FileID, Resnap> resnaps_;
  std::string construct_ = "source construct";
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_EDIT_SINK_H_
