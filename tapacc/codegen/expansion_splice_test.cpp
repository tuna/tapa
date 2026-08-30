// Route-1 mechanics for plan §3.5 (selective macro expansion): record the
// preprocessed token stream in-process, then splice the expansion of the
// OUTERMOST macro invocation that owns a rewrite, applying the rewrite inside
// the spliced text. This file pins the Clang-API facts MF4 depends on:
//
//   1. `clang::syntax::TokenCollector` (Clang 22) records every expanded
//      token of the TU during a normal parse and maps each run of them to
//      the outermost spelled invocation that produced it -- the bookkeeping
//      §3.5 asks for, nested macros included.
//   2. An edit anchored at an expansion SourceLocation maps onto a token
//      index of that invocation (binary search in translation-unit order),
//      so a rewrite expressed in AST coordinates lands inside the
//      reconstructed text.
//   3. Rendering joins token spellings with single spaces: valid (distinct
//      tokens never merge across a space) and deterministic.
//   4. The splice is one file-level replacement followed by a `#line`
//      re-snap, so later diagnostics keep pointing at user source and
//      neighboring invocations stay byte-identical.
//
// The corpus mirrors the real one: the tapa-lib loop macro
// (TAPA_WHILE_NOT_EOT's shape) under a user macro, and a user
// `.invoke(...)` macro.

#include <algorithm>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/AST/Stmt.h"
#include "clang/Basic/LangOptions.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendAction.h"
#include "clang/Lex/Lexer.h"
#include "clang/Lex/Preprocessor.h"
#include "clang/Tooling/Syntax/Tokens.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/ADT/ArrayRef.h"
#include "llvm/ADT/StringRef.h"

#include "frontend/tapa_stub_decls.h"

namespace tapa::cc {
namespace {

// The embedded fixtures use the `tapa` raw-string delimiter, not `cpp`:
// clang-format maps C++ delimiters to the C++ language and reflows their
// content (joining a deliberately split macro invocation, for one), which
// the line-sensitive assertions below cannot tolerate. No language is
// mapped to `tapa`, so the fixtures stay byte-stable under formatting.

using clang::CharSourceRange;
using clang::SourceLocation;
using clang::SourceManager;
using clang::SourceRange;

// The reconstructed expansion of one outermost macro invocation: the
// invocation's spelled range, the expanded tokens it produced, and the edits
// to apply between them.
class ExpansionSplice {
 public:
  ExpansionSplice(const SourceManager& sm, CharSourceRange invocation,
                  llvm::ArrayRef<clang::syntax::Token> tokens)
      : sm_(sm),
        invocation_(invocation),
        tokens_(tokens),
        slots_(tokens.size() + 1) {}

  const CharSourceRange& invocation() const { return invocation_; }

  // InsertTextBefore: text goes immediately before the first expanded token
  // at or after `loc`. This is the shape of `AddPragmaToBody` on a
  // non-compound region -- `_Pragma("...")` before the `if` that an unbraced
  // loop macro produced.
  bool InsertBefore(SourceLocation loc, llvm::StringRef text) {
    const std::optional<size_t> at = IndexAtOrAfter(loc);
    if (!at) return false;
    Append(*at, text);
    return true;
  }

  // InsertTextAfterToken: text goes immediately after the last expanded
  // token at or before `loc`.
  bool InsertAfter(SourceLocation loc, llvm::StringRef text) {
    const std::optional<size_t> at = IndexAtOrBefore(loc);
    if (!at) return false;
    Append(*at + 1, text);
    return true;
  }

  // ReplaceText: the tokens covering [begin, end] are dropped and `text`
  // replaces them.
  bool Replace(SourceLocation begin, SourceLocation end, llvm::StringRef text) {
    const std::optional<size_t> first = IndexAtOrAfter(begin);
    const std::optional<size_t> last = IndexAtOrBefore(end);
    if (!first || !last || *first > *last) return false;
    for (size_t i = *first; i <= *last; ++i) dropped_.insert(i);
    Append(*first, text);
    return true;
  }

  // Deterministic rendering: token spellings joined by single spaces,
  // inserted text keeping its own newlines. A pure function of the recorded
  // spellings and the edit list.
  std::string Render() const {
    std::string out;
    for (size_t i = 0; i < tokens_.size(); ++i) {
      AppendPiece(out, slots_[i]);
      if (dropped_.count(i)) continue;
      out += tokens_[i].text(sm_).str();
      out += ' ';
    }
    AppendPiece(out, slots_[tokens_.size()]);
    while (!out.empty() && out.back() == ' ') out.pop_back();
    return out;
  }

 private:
  static void AppendPiece(std::string& out, const std::string& piece) {
    if (piece.empty()) return;
    out += piece;
    if (piece.back() != ' ' && piece.back() != '\n') out += ' ';
  }

  void Append(size_t slot, llvm::StringRef text) {
    std::string& piece = slots_[slot];
    if (!piece.empty() && piece.back() != '\n') piece += ' ';
    piece += text.str();
  }

  // First expanded token at or after `loc`, in translation-unit order.
  std::optional<size_t> IndexAtOrAfter(SourceLocation loc) const {
    for (size_t i = 0; i < tokens_.size(); ++i) {
      if (tokens_[i].location() == loc) return i;
    }
    const auto* it = std::lower_bound(
        tokens_.begin(), tokens_.end(), loc,
        [&](const clang::syntax::Token& t, SourceLocation l) {
          return sm_.isBeforeInTranslationUnit(t.location(), l);
        });
    if (it == tokens_.end()) return std::nullopt;
    return static_cast<size_t>(it - tokens_.begin());
  }

  // Last expanded token at or before `loc`.
  std::optional<size_t> IndexAtOrBefore(SourceLocation loc) const {
    const std::optional<size_t> after = IndexAtOrAfter(loc);
    if (!after) {
      if (tokens_.empty()) return std::nullopt;
      return tokens_.size() - 1;
    }
    if (tokens_[*after].location() == loc) return *after;
    if (*after == 0) return std::nullopt;
    return *after - 1;
  }

  const SourceManager& sm_;
  CharSourceRange invocation_;
  llvm::ArrayRef<clang::syntax::Token> tokens_;
  std::vector<std::string> slots_;
  std::set<size_t> dropped_;
};

// Route 1's recording side. The collector is installed before
// preprocessing starts and consumed at the top of HandleTranslationUnit:
// ParseAST runs the whole top-level loop (hence the eof token `consume()`
// requires) before handing control to the consumer, so the expanded stream
// is complete by then.
class RecordedTokens {
 public:
  explicit RecordedTokens(clang::Preprocessor& pp)
      : collector_(std::make_unique<clang::syntax::TokenCollector>(pp)) {}

  const clang::syntax::TokenBuffer& Consume() {
    if (!buffer_) {
      buffer_ = std::make_unique<clang::syntax::TokenBuffer>(
          std::move(*collector_).consume());
    }
    return *buffer_;
  }

 private:
  std::unique_ptr<clang::syntax::TokenCollector> collector_;
  std::unique_ptr<clang::syntax::TokenBuffer> buffer_;
};

// The outermost spelled invocation that owns `loc`. Each
// getImmediateExpansionRange step lands on where that expansion was
// invoked, so the walk ends at the invocation spelled in a file.
CharSourceRange OutermostInvocation(const SourceManager& sm,
                                    SourceLocation loc) {
  CharSourceRange range = sm.getImmediateExpansionRange(loc);
  while (range.getBegin().isMacroID()) {
    range = sm.getImmediateExpansionRange(range.getBegin());
  }
  return range;
}

// The expanded tokens of one outermost invocation, straight from the
// TokenBuffer's own mapping (it records only outermost expansions; the ones
// inside macro arguments or bodies fold into the outer one).
llvm::ArrayRef<clang::syntax::Token> ExpansionOf(
    const clang::syntax::TokenBuffer& tokens, const SourceManager& sm,
    const CharSourceRange& invocation) {
  const clang::syntax::Token* first =
      tokens.spelledTokenContaining(sm.getExpansionLoc(invocation.getBegin()));
  if (first == nullptr) return {};
  const std::optional<clang::syntax::TokenBuffer::Expansion> expansion =
      tokens.expansionStartingAt(first);
  if (!expansion) return {};
  return expansion->Expanded;
}

// Applies one splice to the file text: replace the invocation's spelled
// range with the rendered expansion, then re-snap with `#line` when the line
// count changed (mirrors TreeFileBuffer::ResnapAfterEdit).
std::string ApplySplice(const SourceManager& sm, const clang::LangOptions& lang,
                        const ExpansionSplice& splice) {
  const clang::FileID file = sm.getFileID(splice.invocation().getBegin());
  const llvm::StringRef text = sm.getBufferData(file);
  const unsigned begin = sm.getFileOffset(splice.invocation().getBegin());
  const clang::SourceLocation last = splice.invocation().getEnd();
  const unsigned end =
      sm.getFileOffset(last) + clang::Lexer::MeasureTokenLength(last, sm, lang);
  const std::string rendered = splice.Render();
  const unsigned old_lines = llvm::count(text.substr(begin, end - begin), '\n');
  if (old_lines == llvm::count(llvm::StringRef(rendered), '\n')) {
    return std::string(text.substr(0, begin)) + rendered +
           std::string(text.substr(end));
  }
  unsigned marker_at = std::min(end, static_cast<unsigned>(text.size()));
  if (marker_at < text.size() && text[marker_at] == '\n' &&
      marker_at + 1 < text.size()) {
    marker_at += 1;  // the unchanged newline already ends the last line
  }
  std::string marker;
  if (marker_at < text.size()) {
    const bool needs_newline = !rendered.empty() && rendered.back() != '\n';
    const unsigned line = llvm::count(text.substr(0, marker_at), '\n') + 1;
    marker = std::string(needs_newline ? "\n" : "") + "#line " +
             std::to_string(line) + " \"" +
             sm.getFilename(
                   sm.getLocForStartOfFile(file).getLocWithOffset(marker_at))
                 .str() +
             "\"\n";
  }
  // The marker sits at the first unchanged character after the splice, not
  // at the end of the file: it must pin exactly that text's original line.
  return std::string(text.substr(0, begin)) + rendered +
         std::string(text.substr(end, marker_at - end)) + marker +
         std::string(text.substr(marker_at));
}

// Runs one parse with token recording and hands the AST plus the recorded
// stream to `body`; only plain data may escape, because the AST dies with
// the action.
template <typename F>
bool RunRecorded(const std::string& code, const std::vector<std::string>& args,
                 F body, const std::string& file_name = "input.cc",
                 clang::tooling::FileContentMappings mapped = {}) {
  class Action : public clang::ASTFrontendAction {
   public:
    explicit Action(F body) : body_(body) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      recorded_ = std::make_unique<RecordedTokens>(ci.getPreprocessor());
      return true;
    }

    std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
        clang::CompilerInstance&, llvm::StringRef) override {
      class Consumer : public clang::ASTConsumer {
       public:
        explicit Consumer(Action& owner) : owner_(owner) {}
        void HandleTranslationUnit(clang::ASTContext& ctx) override {
          owner_.ok_ = owner_.body_(ctx, owner_.recorded_->Consume());
        }

       private:
        Action& owner_;
      };
      return std::make_unique<Consumer>(*this);
    }

    bool ok() const { return ok_; }

   private:
    F body_;
    std::unique_ptr<RecordedTokens> recorded_;

   public:
    bool ok_ = false;
  };

  auto action = std::make_unique<Action>(body);
  Action* raw = action.get();
  const bool ran = clang::tooling::runToolOnCodeWithArgs(
      std::move(action), code, args, file_name, "expansion-splice-test",
      std::make_shared<clang::PCHContainerOperations>(), mapped);
  return ran && raw->ok();
}

// The first statement of kind S whose begin location is inside a macro
// expansion.
template <typename S>
const S* FirstMacroStmt(clang::ASTContext& ctx) {
  struct Finder : clang::RecursiveASTVisitor<Finder> {
    const S* found = nullptr;
    bool VisitStmt(clang::Stmt* stmt) {
      if (found == nullptr && llvm::isa<S>(stmt) &&
          stmt->getBeginLoc().isMacroID()) {
        found = llvm::cast<S>(stmt);
      }
      return found == nullptr;
    }
  };
  Finder finder;
  finder.TraverseDecl(ctx.getTranslationUnitDecl());
  return finder.found;
}

// The tapa-lib loop macro's shape: a for+if whose `if` is the region a
// `[[tapa::pipeline]]` pragma lowers into.
constexpr char kLoopMacros[] = R"tapa(
#define NOT_EOT(fifo)                                                \
  for (bool valid_##fifo; !fifo.eot(valid_##fifo) || !valid_##fifo;) \
    if (valid_##fifo)
#define FOR_EACH(fifo) NOT_EOT(fifo)
)tapa";

constexpr char kFifoDecls[] = R"tapa(
struct Fifo {
  bool eot(bool& v) { return v, true; }
  int read() { return 0; }
  void flush() {}
};
)tapa";

// What the FOR_EACH(fifo) invocation must render to, token by token.
constexpr char kForEachExpansion[] =
    "for ( bool valid_fifo ; ! fifo . eot ( valid_fifo ) || ! valid_fifo ; ) "
    "if ( valid_fifo )";

// The production edit this spike must carry: `AddPragmaToBody` on the loop's
// non-compound body inserts a `_Pragma` before its begin location.
bool SpliceLoopWithPipelinePragma(clang::ASTContext& ctx,
                                  const clang::syntax::TokenBuffer& tokens,
                                  std::string* rewritten) {
  const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
  if (loop == nullptr) return false;
  const clang::Stmt* body = loop->getBody();
  if (body == nullptr || !body->getBeginLoc().isMacroID()) return false;
  const SourceManager& sm = ctx.getSourceManager();
  const CharSourceRange invocation =
      OutermostInvocation(sm, body->getBeginLoc());
  ExpansionSplice splice(sm, invocation, ExpansionOf(tokens, sm, invocation));
  if (!splice.InsertBefore(body->getBeginLoc(),
                           "_Pragma(\"HLS pipeline II = 1\")")) {
    return false;
  }
  *rewritten = ApplySplice(sm, ctx.getLangOpts(), splice);
  return true;
}

// (b) A TAPA construct inside a NESTED user macro: the outermost invocation
// is the user's FOR_EACH, the loop shape comes from NOT_EOT inside it.
constexpr char kNestedCode[] = R"tapa(
void Task(Fifo& fifo) {
  [[tapa::pipeline(1)]] FOR_EACH(fifo) { fifo.read(); }
}
)tapa";

TEST(ExpansionSplice, OutermostInvocationOfNestedMacroIsTheUserOne) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls + kNestedCode;
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        if (loop == nullptr) return false;
        const SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, loop->getBeginLoc());
        if (!invocation.getBegin().isFileID()) return false;
        // NOT_EOT is spelled inside FOR_EACH's body; the outermost
        // invocation is the user-level spelling.
        const std::string spelled =
            clang::Lexer::getSourceText(invocation, sm, ctx.getLangOpts())
                .str();
        if (spelled != "FOR_EACH(fifo)") return false;
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        return splice.Render() == kForEachExpansion;
      });
  EXPECT_TRUE(ok);
}

TEST(ExpansionSplice, PragmaInsertionLandsInsideSplicedExpansion) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls + kNestedCode;
  std::string rewritten;
  ASSERT_TRUE(RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        return SpliceLoopWithPipelinePragma(ctx, tokens, &rewritten);
      }));
  // The invocation is gone (only the `#define FOR_EACH` line still spells
  // it), its expansion is spliced in, and the pragma sits immediately before
  // the `if` of the expansion.
  EXPECT_NE(rewritten.find("  [[tapa::pipeline(1)]] for ( bool valid_fifo ; "
                           "! fifo . eot ( valid_fifo ) || ! valid_fifo ; ) "
                           "_Pragma(\"HLS pipeline II = 1\") if "
                           "( valid_fifo ) { fifo.read(); }"),
            std::string::npos)
      << rewritten;
  EXPECT_NE(rewritten.find("_Pragma(\"HLS pipeline II = 1\") if"),
            std::string::npos);
  EXPECT_NE(rewritten.find("for ( bool valid_fifo ;"), std::string::npos);
  // The user's braces survive verbatim (they are outside the invocation).
  EXPECT_NE(rewritten.find("{ fifo.read(); }"), std::string::npos);
}

// (c) A neighboring invocation on the same line stays spelled: the splice is
// bounded by exactly the rewritten invocation's tokens.
TEST(ExpansionSplice, NeighborInvocationOnTheSameLineIsUntouched) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls + R"tapa(
#define FLUSH(fifo) fifo.flush()
void Task(Fifo& fifo) {
  [[tapa::pipeline(1)]] FOR_EACH(fifo) { fifo.read(); } FLUSH(fifo);
}
)tapa";
  std::string rewritten;
  ASSERT_TRUE(RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        return SpliceLoopWithPipelinePragma(ctx, tokens, &rewritten);
      }));
  EXPECT_NE(rewritten.find("FLUSH(fifo);"), std::string::npos);
  EXPECT_EQ(rewritten.find("fifo.flush();"), std::string::npos);
}

// (a) A task invocation wrapped in a user macro (the multi-file app's
// INVOKE_CONSUME). The graph side already sees through the macro; this pins
// the rewrite side for an edit whose location the macro owns -- here an
// attributed statement over the invoke chain.
TEST(ExpansionSplice, InvokeMacroSpliceAppliesEditInsideExpansion) {
  const std::string code = std::string(kTapaStubDecls) + R"tapa(
#define INVOKE_CONSUME(s, m, n) .invoke(Consume, s, m, n)

void Consume(tapa::istream<float>& in, tapa::mmap<float> mem,
             unsigned long long n) {
  for (unsigned long long i = 0; i < n; ++i) mem[i] = in.read();
}

void Top(tapa::mmap<const float> a, tapa::mmap<float> c,
         unsigned long long n) {
  tapa::stream<float> q;
  tapa::task()
      .invoke(Consume, q, c, n) INVOKE_CONSUME(q, c, n);
}
)tapa";
  std::string rewritten;
  std::string why;
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::CXXMemberCallExpr* invoke = nullptr;
        const clang::SourceManager& sm = ctx.getSourceManager();
        // NOTE: the member call's own begin is the receiver expression,
        // which lives in the file -- only the callee (the `.invoke` token)
        // is inside the expansion. An EditSink must therefore look at the
        // edit's own location, not at the node's range begin, to decide
        // that a rewrite is macro-owned.
        struct Finder : clang::RecursiveASTVisitor<Finder> {
          const clang::CXXMemberCallExpr* found = nullptr;
          bool VisitCXXMemberCallExpr(clang::CXXMemberCallExpr* call) {
            if (found == nullptr && call->getExprLoc().isMacroID()) {
              found = call;
            }
            return found == nullptr;
          }
        } finder;
        finder.TraverseDecl(ctx.getTranslationUnitDecl());
        invoke = finder.found;
        if (invoke == nullptr) {
          why = "no macro invoke found";
          return false;
        }
        const CharSourceRange invocation =
            OutermostInvocation(sm, invoke->getExprLoc());
        const std::string spelled =
            clang::Lexer::getSourceText(invocation, sm, ctx.getLangOpts())
                .str();
        if (spelled != "INVOKE_CONSUME(q, c, n)") {
          why = "outermost invocation is: " + spelled;
          return false;
        }
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        if (!splice.InsertBefore(invoke->getExprLoc(), "/*rewritten*/")) {
          return false;
        }
        rewritten = ApplySplice(sm, ctx.getLangOpts(), splice);
        return true;
      });
  ASSERT_TRUE(ok) << why;
  EXPECT_EQ(rewritten.find("INVOKE_CONSUME(q, c, n)"), std::string::npos);
  // The expansion is spliced in with the edit immediately before the
  // `.invoke` member name.
  const size_t edit = rewritten.find("/*rewritten*/");
  ASSERT_NE(edit, std::string::npos) << rewritten;
  EXPECT_LT(edit, rewritten.find("invoke ( Consume , q , c , n )"));
}

// (d) The `#line` re-snap pins the text after the splice to its original
// line, so a vendor diagnostic there cites user source.
TEST(ExpansionSplice, LineResnapPinsFollowingTextToOriginalNumbering) {
  // The invocation spans two lines, so splicing its one-line expansion in
  // changes the line count and must be followed by a re-snap marker.
  const std::string code = std::string(kLoopMacros) + kFifoDecls + R"tapa(
void Task(Fifo& fifo) {
  [[tapa::pipeline(1)]] FOR_EACH(
      fifo) { fifo.read(); }
}
void After(Fifo& fifo) { fifo.flush(); }
)tapa";
  std::string rewritten;
  ASSERT_TRUE(RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        return SpliceLoopWithPipelinePragma(ctx, tokens, &rewritten);
      }))
      << rewritten;
  // A diagnostic after the splice must still cite user source: replay the
  // marker the way any compiler's line tracking does, and check that the
  // line it assigns `void After` is the one the ORIGINAL file has it on.
  const size_t after = code.find("void After");
  ASSERT_NE(after, std::string::npos);
  const unsigned original_line =
      static_cast<unsigned>(
          llvm::count(llvm::StringRef(code).substr(0, after), '\n')) +
      1;
  const size_t marker = rewritten.find("#line ");
  ASSERT_NE(marker, std::string::npos) << rewritten;
  const unsigned marker_line =
      static_cast<unsigned>(std::stoul(rewritten.substr(marker + 6)));
  const size_t after_marker = rewritten.find('\n', marker) + 1;
  const size_t after_in_rewritten = rewritten.find("void After");
  ASSERT_LT(after_marker, after_in_rewritten);
  const unsigned effective_line =
      marker_line + static_cast<unsigned>(llvm::count(
                        llvm::StringRef(rewritten).substr(
                            after_marker, after_in_rewritten - after_marker),
                        '\n'));
  EXPECT_EQ(effective_line, original_line);
}

// The InsertTextAfterToken shape, on the macro-owned task body that
// rewrite_test's MacroOwnedEditIsAHardError pins today: a lower-level task
// inserts its preamble after the `{` the macro produced.
constexpr char kMacroBody[] = R"tapa(
#define BODY \
  {          \
    fifo.read(); \
  }
void Lower(Fifo& fifo) BODY
)tapa";

TEST(ExpansionSplice, MacroOwnedBodyCarriesInsertAfterToken) {
  const std::string code = std::string(kFifoDecls) + kMacroBody;
  std::string rewritten;
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::CompoundStmt* body = nullptr;
        struct Finder : clang::RecursiveASTVisitor<Finder> {
          const clang::CompoundStmt** found;
          bool VisitCompoundStmt(clang::CompoundStmt* compound) {
            if (*found == nullptr && compound->getLBracLoc().isMacroID()) {
              *found = compound;
            }
            return *found == nullptr;
          }
        } finder;
        finder.found = &body;
        finder.TraverseDecl(ctx.getTranslationUnitDecl());
        if (body == nullptr) return false;
        const SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, body->getLBracLoc());
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        if (!splice.InsertAfter(body->getLBracLoc(),
                                "\n#pragma HLS interface ap_fifo\n")) {
          return false;
        }
        rewritten = ApplySplice(sm, ctx.getLangOpts(), splice);
        return true;
      });
  ASSERT_TRUE(ok);
  // The inserted text keeps its own newlines; the expansion's tokens are
  // space-joined.
  EXPECT_NE(rewritten.find("void Lower(Fifo& fifo) { \n"
                           "#pragma HLS interface ap_fifo\n"
                           "fifo . read ( ) ; }"),
            std::string::npos)
      << rewritten;
}

// The ReplaceText shape: a middle-level task's macro-owned body is replaced
// by an interface shell (RewriteTaskFunc on kMiddle).
TEST(ExpansionSplice, MacroOwnedBodyCarriesReplace) {
  const std::string code = std::string(kFifoDecls) + kMacroBody;
  std::string rewritten;
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::CompoundStmt* body = nullptr;
        struct Finder : clang::RecursiveASTVisitor<Finder> {
          const clang::CompoundStmt** found;
          bool VisitCompoundStmt(clang::CompoundStmt* compound) {
            if (*found == nullptr && compound->getLBracLoc().isMacroID()) {
              *found = compound;
            }
            return *found == nullptr;
          }
        } finder;
        finder.found = &body;
        finder.TraverseDecl(ctx.getTranslationUnitDecl());
        if (body == nullptr) return false;
        const SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, body->getBeginLoc());
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        if (!splice.Replace(body->getBeginLoc(), body->getEndLoc(),
                            "{\n#pragma HLS interface ap_none\n}\n")) {
          return false;
        }
        rewritten = ApplySplice(sm, ctx.getLangOpts(), splice);
        return true;
      });
  ASSERT_TRUE(ok);
  EXPECT_NE(rewritten.find("void Lower(Fifo& fifo) {\n#pragma HLS interface "
                           "ap_none\n}\n"),
            std::string::npos)
      << rewritten;
  // Only the `#define BODY` line still spells the old body.
  EXPECT_EQ(rewritten.find("BODY\n  {\n    fifo.read();\n  }\n"),
            std::string::npos);
}

// (e) A shared header seen from two TUs: same invocation, same arguments,
// same defines -> byte-identical expansion text, which is what the §3.4
// byte-identical-header tripwire needs to hold.
std::string SharedHeaderExpansion(const std::vector<std::string>& args) {
  std::string rendered;
  RunRecorded(
      R"tapa(#include "shared.h"
void Tu(Fifo& fifo) {
  [[tapa::pipeline(1)]] FOR_EACH(fifo) { fifo.read(); }
}
)tapa",
      args,
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        if (loop == nullptr) return false;
        const SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, loop->getBody()->getBeginLoc());
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        rendered = splice.Render();
        return true;
      },
      /*file_name=*/"tu.cc",
      {std::make_pair(std::string("shared.h"),
                      std::string(kLoopMacros) + kFifoDecls)});
  return rendered;
}

TEST(ExpansionSplice, SharedHeaderExpansionIsByteIdenticalAcrossTus) {
  const std::string a = SharedHeaderExpansion({"-std=c++17"});
  const std::string b = SharedHeaderExpansion({"-std=c++17", "-Dextra=1"});
  ASSERT_EQ(a, kForEachExpansion);
  // An unrelated define cannot change the expansion; the two TUs' headers
  // rewrite to the same bytes.
  EXPECT_EQ(a, b);
}

// (e, divergence half) A TU that makes the header's macro expand differently
// yields different expansion text, and the difference is visible in that
// text -- so the §3.4 tripwire can report the precise reason instead of a
// bare "headers diverge".
constexpr char kLimitMacros[] = R"tapa(
#ifndef LIMIT
#define LIMIT 8
#endif
#define PAD(name) char name##_pad[LIMIT]
)tapa";

std::string PadExpansion(const std::vector<std::string>& args) {
  std::string rendered;
  RunRecorded(
      R"tapa(#include "shared.h"
void Tu() {
  PAD(buf);
}
)tapa",
      args,
      [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::DeclStmt* decl = FirstMacroStmt<clang::DeclStmt>(ctx);
        if (decl == nullptr) return false;
        const SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, decl->getBeginLoc());
        ExpansionSplice splice(sm, invocation,
                               ExpansionOf(tokens, sm, invocation));
        rendered = splice.Render();
        return true;
      },
      "tu.cc",
      {std::make_pair(std::string("shared.h"), std::string(kLimitMacros))});
  return rendered;
}

TEST(ExpansionSplice, DivergingDefinesYieldDivergingExpansionText) {
  const std::string eight = PadExpansion({"-std=c++17"});
  const std::string sixteen = PadExpansion({"-std=c++17", "-DLIMIT=16"});
  ASSERT_EQ(eight, "char buf_pad [ 8 ]");
  // The second TU's define enters the same header's expansion, so the
  // rewritten bytes differ -- exactly where, is readable in the text.
  ASSERT_EQ(sixteen, "char buf_pad [ 16 ]");
}

}  // namespace
}  // namespace tapa::cc
