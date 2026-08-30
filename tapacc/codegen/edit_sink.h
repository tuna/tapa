#ifndef TAPA_CODEGEN_EDIT_SINK_H_
#define TAPA_CODEGEN_EDIT_SINK_H_

#include <string>

#include "clang/Basic/LangOptions.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Rewrite/Core/Rewriter.h"
#include "llvm/ADT/StringRef.h"

namespace tapa::cc {

// The edit surface every decl-rewrite rule writes through: the exact
// clang::Rewriter calls the rules make, behind one type, so the same rules
// serve both producers. The flattened producer builds it over one bare
// Rewriter and edits apply verbatim; the rewritten-tree producer layers
// per-file `#line` re-snap bookkeeping and macro guards on the same calls.
class EditSink {
 public:
  explicit EditSink(clang::Rewriter& rewriter) : rewriter_(rewriter) {}

  bool ReplaceText(clang::SourceRange range, llvm::StringRef text) {
    return rewriter_.ReplaceText(range, text);
  }
  bool ReplaceText(clang::SourceLocation start, unsigned length,
                   llvm::StringRef text) {
    return rewriter_.ReplaceText(start, length, text);
  }
  bool InsertTextBefore(clang::SourceLocation loc, llvm::StringRef text) {
    return rewriter_.InsertTextBefore(loc, text);
  }
  bool InsertTextAfter(clang::SourceLocation loc, llvm::StringRef text) {
    return rewriter_.InsertTextAfter(loc, text);
  }
  bool InsertTextAfterToken(clang::SourceLocation loc, llvm::StringRef text) {
    return rewriter_.InsertTextAfterToken(loc, text);
  }
  bool RemoveText(clang::SourceLocation start, unsigned length) {
    return rewriter_.RemoveText(start, length);
  }
  bool RemoveText(clang::SourceRange range) {
    return rewriter_.RemoveText(range);
  }
  std::string getRewrittenText(clang::SourceRange range) const {
    return rewriter_.getRewrittenText(range);
  }
  std::string getRewrittenText(clang::CharSourceRange range) const {
    return rewriter_.getRewrittenText(range);
  }
  clang::SourceManager& getSourceMgr() const {
    return rewriter_.getSourceMgr();
  }
  const clang::LangOptions& getLangOpts() const {
    return rewriter_.getLangOpts();
  }

 private:
  clang::Rewriter& rewriter_;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_EDIT_SINK_H_
