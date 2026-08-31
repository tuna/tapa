#ifndef TAPA_CODEGEN_MACRO_SPLICE_H_
#define TAPA_CODEGEN_MACRO_SPLICE_H_

#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "clang/Basic/SourceLocation.h"
#include "clang/Tooling/Syntax/Tokens.h"
#include "llvm/ADT/ArrayRef.h"
#include "llvm/ADT/StringRef.h"

namespace clang {
class Preprocessor;
class SourceManager;
}  // namespace clang

namespace tapa::cc {

// Selective macro expansion (plan §3.5, route 1): a rewrite edit anchored
// inside a macro expansion is composed into token slots on the OUTERMOST
// spelled invocation that owns it; at session end that invocation's spelled
// range is replaced once by the rendered expansion, so invocations holding
// no rewrite stay spelled and neighbors on the same line are untouched.

// Records one TU's expanded token stream during a normal parse. Install in
// BeginSourceFileAction (before preprocessing starts) and Consume() exactly
// once at the top of HandleTranslationUnit: ParseAST finishes the whole
// top-level loop -- hence the eof token consume() requires -- before handing
// control to the consumer, so the stream is complete by then. Only the
// rewrite pass records; the index pass never edits.
class TokenRecorder {
 public:
  explicit TokenRecorder(clang::Preprocessor& pp);

  const clang::syntax::TokenBuffer& Consume();

 private:
  std::unique_ptr<clang::syntax::TokenCollector> collector_;
  std::unique_ptr<clang::syntax::TokenBuffer> buffer_;
};

// The outermost spelled invocation owning `loc` (a macro location): each
// getImmediateExpansionRange step lands where that expansion was invoked,
// so the walk ends at the invocation spelled in a file -- the user-level
// spelling, not a nested macro's.
clang::CharSourceRange OutermostInvocation(const clang::SourceManager& sm,
                                           clang::SourceLocation loc);

// One invocation under splice: the recorded expanded tokens with an edit
// slot before each one (and after the last). Edits compose deterministically
// in arrival order; token ranges replaced or removed by one edit may not
// overlap another's. Rendering joins kept token spellings with single
// spaces -- valid (distinct tokens never merge across a space) and a pure
// function of the recorded spellings and the edit list.
class ExpansionSplice {
 public:
  ExpansionSplice(const clang::SourceManager& sm,
                  clang::CharSourceRange invocation,
                  llvm::ArrayRef<clang::syntax::Token> tokens);

  const clang::CharSourceRange& invocation() const { return invocation_; }
  bool has_edits() const { return edited_; }

  // InsertTextBefore: text immediately before the first token at or after
  // `loc`.
  bool InsertBefore(clang::SourceLocation loc, llvm::StringRef text,
                    std::string* why);
  // InsertTextAfter/InsertTextAfterToken: text immediately after the last
  // token at or before `loc`.
  bool InsertAfter(clang::SourceLocation loc, llvm::StringRef text,
                   std::string* why);
  // ReplaceText: the tokens covering [begin, end] are dropped and `text`
  // takes their place. RemoveText is ReplaceText with empty text.
  bool Replace(clang::SourceLocation begin, clang::SourceLocation end,
               llvm::StringRef text, std::string* why);

  // Drop tokens [first, last] inclusive, substituting `text`: the
  // coordinates callers work in after inspecting neighbor tokens
  // themselves (macro-owned attribute removal).
  bool DropTokens(size_t first, size_t last, llvm::StringRef text,
                  std::string* why);

  // Token introspection in invocation coordinates; nullopt when `loc` is
  // outside the invocation's recorded tokens.
  std::optional<size_t> IndexAtOrAfter(clang::SourceLocation loc) const;
  std::optional<size_t> IndexAtOrBefore(clang::SourceLocation loc) const;
  llvm::StringRef TokenText(size_t index) const;
  size_t size() const { return tokens_.size(); }

  std::string Render() const;

 private:
  void Append(size_t slot, llvm::StringRef text);

  const clang::SourceManager& sm_;
  clang::CharSourceRange invocation_;
  llvm::ArrayRef<clang::syntax::Token> tokens_;
  std::vector<std::string> slots_;
  std::set<size_t> dropped_;
  bool edited_ = false;
};

// Every splice of one TU, one per rewritten invocation, keyed by its spelled
// invocation begin. Creating a splice for an invocation the token buffer
// holds no mapping of is the residual impossible case and fails.
class MacroSplices {
 public:
  MacroSplices(const clang::syntax::TokenBuffer& tokens,
               const clang::SourceManager& sm);

  // The splice owning `loc` (a macro location), creating it on first sight.
  // Nullptr and a precise `why` when the invocation has no recorded
  // expansion.
  ExpansionSplice* SpliceFor(clang::SourceLocation loc, std::string* why);

  // The splices carrying edits, in invocation source order.
  std::vector<ExpansionSplice*> Edited();

 private:
  const clang::syntax::TokenBuffer& tokens_;
  const clang::SourceManager& sm_;
  std::map<clang::SourceLocation, std::unique_ptr<ExpansionSplice>> splices_;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_MACRO_SPLICE_H_
