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

namespace tapa::cc {

// The edit surface every decl-rewrite rule writes through: the exact
// clang::Rewriter calls the rules make, behind one type, so the same rules
// serve both producers. The flattened producer builds it over one bare
// Rewriter and edits apply verbatim. The rewritten-tree producer additionally
// registers one tracker per mirrored file: every edit then gets `#line`
// re-snap bookkeeping, while a rewrite inside a macro expansion or outside
// the mirror is a hard diagnostic.
class EditSink {
 public:
  using Resnap =
      std::function<bool(clang::SourceLocation, unsigned, llvm::StringRef)>;

  // Flattened producer: forward every edit verbatim.
  explicit EditSink(clang::Rewriter& rewriter);

  // Rewritten-tree producer: rejected edits report through `ctx`; call
  // TrackFile for every mirrored FileID before applying rewrite rules.
  EditSink(clang::ASTContext& ctx, clang::Rewriter& rewriter);

  void TrackFile(clang::FileID file, Resnap resnap);
  void Describe(std::string construct) { construct_ = std::move(construct); }

  bool ReplaceText(clang::SourceRange range, llvm::StringRef text);
  bool ReplaceText(clang::SourceLocation start, unsigned length,
                   llvm::StringRef text);
  bool InsertTextBefore(clang::SourceLocation loc, llvm::StringRef text);
  bool InsertTextAfter(clang::SourceLocation loc, llvm::StringRef text);
  bool InsertTextAfterToken(clang::SourceLocation loc, llvm::StringRef text);
  bool RemoveText(clang::SourceLocation start, unsigned length);
  bool RemoveText(clang::SourceRange range);

  std::string getRewrittenText(clang::SourceRange range) const;
  std::string getRewrittenText(clang::CharSourceRange range) const;
  clang::SourceManager& getSourceMgr() const;
  const clang::LangOptions& getLangOpts() const;

 private:
  bool CanEdit(clang::SourceLocation begin, clang::SourceLocation end);
  bool Finish(clang::SourceLocation begin, unsigned length,
              llvm::StringRef text, bool rejected);
  unsigned OriginalLength(clang::CharSourceRange range) const;

  clang::ASTContext* ctx_ = nullptr;
  clang::Rewriter& rewriter_;
  std::map<clang::FileID, Resnap> resnaps_;
  std::string construct_ = "source construct";
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_EDIT_SINK_H_
