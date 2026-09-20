// API-level pins for macro_splice.h (selective macro expansion): the
// recorded token stream maps every edit anchored in an
// expansion to its OUTERMOST spelled invocation, edits compose into token
// slots deterministically, and rendering is a pure function of the recorded
// spellings. The production rewrite path over these APIs lives in
// macro_rewrite_test.cpp; this file pins the mechanics themselves.
//
// The embedded fixtures use the `tapa` raw-string delimiter, not `cpp`:
// clang-format maps C++ delimiters to the C++ language and reflows their
// content (joining a deliberately split macro invocation, for one), which
// the line-sensitive assertions cannot tolerate. No language is mapped to
// `tapa`, so the fixtures stay byte-stable under formatting.

#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/AST/Stmt.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendAction.h"
#include "clang/Lex/Lexer.h"
#include "clang/Tooling/Syntax/Tokens.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/ADT/StringRef.h"

#include "frontend/tapa_stub_decls.h"
#include "macro_splice.h"

namespace tapa::cc {
namespace {

using clang::CharSourceRange;
using clang::SourceLocation;

// Runs one parse with token recording (exactly how the rewrite pass
// installs the collector) and hands the AST plus the recorded stream to
// `body`; only plain data may escape, because the AST dies with the action.
template <typename F>
bool RunRecorded(const std::string& code, const std::vector<std::string>& args,
                 F body, const std::string& file_name = "input.cc",
                 clang::tooling::FileContentMappings mapped = {}) {
  class Action : public clang::ASTFrontendAction {
   public:
    explicit Action(F body) : body_(body) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      recorded_ = std::make_unique<TokenRecorder>(ci.getPreprocessor());
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
    std::unique_ptr<TokenRecorder> recorded_;

   public:
    bool ok_ = false;
  };

  auto action = std::make_unique<Action>(body);
  Action* raw = action.get();
  const bool ran = clang::tooling::runToolOnCodeWithArgs(
      std::move(action), code, args, file_name, "expansion-splice-test",
      std::make_shared<clang::PCHContainerOperations>(), mapped);
  return ran && raw->ok_;
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
// `[[tapa::pipeline]]` pragma lowers into, nested inside a user macro the
// way FOR_EACH nests NOT_EOT.
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

// The splice of the outermost invocation owning a macro-owned statement.
ExpansionSplice SpliceOf(clang::ASTContext& ctx,
                         const clang::syntax::TokenBuffer& tokens,
                         clang::SourceLocation loc) {
  const clang::SourceManager& sm = ctx.getSourceManager();
  const CharSourceRange invocation = OutermostInvocation(sm, loc);
  std::string why;
  MacroSplices splices(tokens, sm);
  ExpansionSplice* splice = splices.SpliceFor(loc, &why);
  return splice == nullptr ? ExpansionSplice(sm, invocation, {})
                           : std::move(*splice);
}

// A macro construct nested in a user macro resolves to the USER-level
// spelling, and the recorded expansion carries the whole for+if.
TEST(ExpansionSplice, OutermostInvocationOfNestedMacroIsTheUserOne) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls +
                           R"tapa(
void Task(Fifo& fifo) {
  FOR_EACH(fifo) { fifo.read(); }
}
)tapa";
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        if (loop == nullptr) return false;
        const clang::SourceManager& sm = ctx.getSourceManager();
        const CharSourceRange invocation =
            OutermostInvocation(sm, loop->getBeginLoc());
        if (!invocation.getBegin().isFileID()) return false;
        // NOT_EOT is spelled inside FOR_EACH's body; the outermost
        // invocation is the user-level spelling.
        const std::string spelled =
            clang::Lexer::getSourceText(invocation, sm, ctx.getLangOpts())
                .str();
        if (spelled != "FOR_EACH(fifo)") return false;
        return SpliceOf(ctx, tokens, loop->getBeginLoc()).Render() ==
               kForEachExpansion;
      });
  EXPECT_TRUE(ok);
}

// The three edit shapes on token slots: an insert before a token, an
// insert after a token, and a replacement dropping a token range.
TEST(ExpansionSplice, TokenSlotEditsRenderDeterministically) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls +
                           R"tapa(
void Task(Fifo& fifo) {
  FOR_EACH(fifo) { fifo.read(); }
}
)tapa";
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        const clang::Stmt* body = loop == nullptr ? nullptr : loop->getBody();
        if (body == nullptr) return false;
        ExpansionSplice splice = SpliceOf(ctx, tokens, body->getBeginLoc());
        std::string why;
        if (!splice.InsertBefore(body->getBeginLoc(), "_Pragma(\"one\")",
                                 &why) ||
            !splice.InsertAfter(body->getBeginLoc(), "/*after*/", &why)) {
          return false;
        }
        // Replace the `for (...)` condition: drop the tokens of the
        // condition expression (eot call through the || operand).
        const clang::Expr* cond = loop->getCond();
        if (!splice.Replace(cond->getBeginLoc(), cond->getEndLoc(), "c", &why))
          return false;
        return splice.Render() ==
               "for ( bool valid_fifo ; c ; ) "
               "_Pragma(\"one\") if /*after*/ ( valid_fifo )";
      });
  EXPECT_TRUE(ok) << "slot edits compose in arrival order";
}

// Two replacements claiming one token have no single honest rendering;
// the second is rejected with a reason, and an insert at the same slot
// still composes.
TEST(ExpansionSplice, OverlappingReplacementsAreRejected) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls +
                           R"tapa(
void Task(Fifo& fifo) {
  FOR_EACH(fifo) { fifo.read(); }
}
)tapa";
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        const clang::Stmt* body = loop == nullptr ? nullptr : loop->getBody();
        if (body == nullptr) return false;
        ExpansionSplice splice = SpliceOf(ctx, tokens, body->getBeginLoc());
        std::string why;
        const clang::Expr* cond = loop->getCond();
        if (!splice.Replace(cond->getBeginLoc(), cond->getEndLoc(), "a", &why))
          return false;
        if (splice.Replace(cond->getBeginLoc(), cond->getEndLoc(), "b", &why)) {
          return false;  // must be rejected
        }
        if (why.find("overlaps") == std::string::npos) return false;
        // A narrower replacement inside already-dropped tokens is the same
        // conflict.
        const clang::Expr* call = llvm::cast<clang::Expr>(*cond->child_begin());
        if (splice.Replace(call->getBeginLoc(), call->getEndLoc(), "c", &why))
          return false;
        return splice.InsertBefore(body->getBeginLoc(), "_Pragma(\"x\")", &why);
      });
  EXPECT_TRUE(ok);
}

// An anchor outside the invocation's recorded tokens has no honest slot:
// the edit is rejected, never silently mapped onto the nearest token.
TEST(ExpansionSplice, AnchorsOutsideTheInvocationAreRejected) {
  const std::string code = std::string(kLoopMacros) + kFifoDecls +
                           R"tapa(
void Before(Fifo& fifo) { fifo.flush(); }
void Task(Fifo& fifo) {
  FOR_EACH(fifo) { fifo.read(); }
}
)tapa";
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        // A file location before and one after the invocation: the
        // enclosing function's own begin and end.
        const clang::FunctionDecl* task = nullptr;
        struct FindFunc : clang::RecursiveASTVisitor<FindFunc> {
          const clang::FunctionDecl** found;
          bool VisitFunctionDecl(clang::FunctionDecl* decl) {
            if (*found == nullptr && decl->getNameAsString() == "Task") {
              *found = decl;
            }
            return *found == nullptr;
          }
        } find;
        find.found = &task;
        find.TraverseDecl(ctx.getTranslationUnitDecl());
        if (task == nullptr) return false;
        const clang::SourceLocation before = task->getBeginLoc();
        const clang::SourceLocation after = task->getEndLoc();
        const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
        if (loop == nullptr) return false;
        ExpansionSplice splice = SpliceOf(ctx, tokens, loop->getBeginLoc());
        std::string why;
        if (splice.InsertBefore(before, "x", &why)) return false;
        if (splice.InsertAfter(after, "x", &why)) return false;
        if (why.find("outside") == std::string::npos) return false;
        return true;
      });
  EXPECT_TRUE(ok);
}

// A member-call edit anchors at the callee name's location: the call's own
// range begin is the receiver, which may live in user source while the
// `.invoke` is macro-owned.
TEST(ExpansionSplice, InvokeMacroEditAnchorsAtTheCalleeName) {
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
  const bool ok = RunRecorded(
      code, {"-std=c++17"},
      [](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
        const clang::CXXMemberCallExpr* invoke = nullptr;
        struct Finder : clang::RecursiveASTVisitor<Finder> {
          const clang::CXXMemberCallExpr** found;
          bool VisitCXXMemberCallExpr(clang::CXXMemberCallExpr* call) {
            if (*found == nullptr && call->getExprLoc().isMacroID()) {
              *found = call;
            }
            return *found == nullptr;
          }
        } finder;
        finder.found = &invoke;
        finder.TraverseDecl(ctx.getTranslationUnitDecl());
        if (invoke == nullptr) return false;
        const clang::SourceManager& sm = ctx.getSourceManager();
        const std::string spelled =
            clang::Lexer::getSourceText(
                OutermostInvocation(sm, invoke->getExprLoc()), sm,
                ctx.getLangOpts())
                .str();
        if (spelled != "INVOKE_CONSUME(q, c, n)") return false;
        ExpansionSplice splice = SpliceOf(ctx, tokens, invoke->getExprLoc());
        std::string why;
        if (!splice.InsertBefore(invoke->getExprLoc(), "/*rewritten*/", &why))
          return false;
        return splice.Render().find(
                   "/*rewritten*/ invoke ( Consume , q , "
                   "c , n )") != std::string::npos;
      });
  EXPECT_TRUE(ok);
}

// The render is a pure function of the recorded spellings: the same header
// seen from two parses (two translation units) with no differing defines
// renders byte-identically, which is what the shared-header byte-identity
// tripwire needs to hold.
TEST(ExpansionSplice, RenderIsIdenticalAcrossParsesOfOneHeader) {
  const auto render = [](const std::vector<std::string>& args) {
    std::string rendered;
    RunRecorded(
        std::string(kLoopMacros) + kFifoDecls +
            R"tapa(
void Task(Fifo& fifo) {
  FOR_EACH(fifo) { fifo.read(); }
}
)tapa",
        args,
        [&](clang::ASTContext& ctx, const clang::syntax::TokenBuffer& tokens) {
          const clang::ForStmt* loop = FirstMacroStmt<clang::ForStmt>(ctx);
          if (loop == nullptr) return false;
          rendered = SpliceOf(ctx, tokens, loop->getBeginLoc()).Render();
          return true;
        });
    return rendered;
  };
  EXPECT_EQ(render({"-std=c++17"}), kForEachExpansion);
  EXPECT_EQ(render({"-std=c++17", "-Dextra=1"}), kForEachExpansion);
}

}  // namespace
}  // namespace tapa::cc
