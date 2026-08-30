// Production-path tests for selective macro expansion (plan §3.5): the
// full tree-mode pipeline -- index pass, merge, rewrite pass with the token
// stream recorded, guarded mirror materialization -- over constructs a
// rewrite anchors inside a macro expansion. Each fixture asserts the exact
// rendered bytes, so an edit silently landing outside the spliced expansion
// fails here rather than at HLS. The mechanics behind these paths are pinned
// in expansion_splice_test.cpp.
//
// The fixtures use the `tapa` raw-string delimiter, not `cpp`:
// clang-format maps C++ delimiters to the C++ language and reflows their
// content, which the byte-exact assertions cannot tolerate. No language is
// mapped to `tapa`, so they stay byte-stable under formatting.

#include <dirent.h>

#include <functional>
#include <map>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendAction.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"

#include "diag_capture.h"
#include "frontend/program_builder.h"
#include "frontend/tapa_stub_decls.h"
#include "macro_splice.h"
#include "tree_writer.h"

namespace tapa::cc {
namespace {

// ── Test scaffolding ──────────────────────────────────────────────────────

std::string TempRoot(const std::string& name) {
  const std::string root = testing::TempDir() + "/macro_rewrite_" + name;
  llvm::sys::fs::remove_directories(root);
  EXPECT_FALSE(llvm::sys::fs::create_directories(root));
  return root;
}

std::string ReadFile(const std::string& path) {
  const auto buffer = llvm::MemoryBuffer::getFile(path);
  EXPECT_TRUE(buffer) << path;
  return buffer ? (*buffer)->getBuffer().str() : std::string();
}

std::map<std::string, std::string> ReadTree(const std::string& root) {
  std::map<std::string, std::string> files;
  std::function<void(const std::string&, const std::string&)> walk =
      [&](const std::string& prefix, const std::string& dir) {
        DIR* stream = opendir(dir.c_str());
        ASSERT_NE(stream, nullptr);
        while (dirent* entry = readdir(stream)) {
          const std::string name = entry->d_name;
          if (name == "." || name == "..") continue;
          const std::string path = dir + "/" + name;
          if (entry->d_type == DT_DIR) {
            walk(prefix + name + "/", path);
          } else {
            files[prefix + name] = ReadFile(path);
          }
        }
        closedir(stream);
      };
  walk("", root);
  return files;
}

// One virtual translation unit: path plus code, and optionally its own
// compile flags (a TU-local define is how a shared header's macro can
// expand differently per TU).
struct VirtualTu {
  std::string path;
  std::string code;
  std::vector<std::string> own_args;
  VirtualTu(std::string p, std::string c, std::vector<std::string> a = {})
      : path(std::move(p)), code(std::move(c)), own_args(std::move(a)) {}
};

// The outcome of one pipeline run.
struct TreePipelineRun {
  // The rendered mirrored tree, tree-relative path -> bytes. Empty when the
  // run failed before materialization.
  std::map<std::string, std::string> files;
  // Every error diagnostic of both passes, formatted text.
  std::vector<std::string> diags;
  // The single graph JSON, as tapa-ir consumes it.
  std::string json;
};

// Runs exactly what `tapacc -tree` runs over the virtual TUs: an index
// action per TU (include log, no tokens), the merge, then a rewrite action
// per TU with the token stream recorded, then the tree materialization.
// All TUs parse with `args`, except one carrying its own flags.
TreePipelineRun RunTreePipeline(
    const std::vector<VirtualTu>& tus, const std::string& top,
    const std::string& out_root, const std::vector<std::string>& args,
    const clang::tooling::FileContentMappings& virtual_files = {}) {
  class PipelineAction : public clang::ASTFrontendAction {
   public:
    PipelineAction(ProgramBuilder& builder, bool index_pass,
                   CollectingDiagConsumer* diags)
        : builder_(builder), index_pass_(index_pass), diags_(diags) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      ci.getDiagnostics().setClient(diags_, /*OwnsClient=*/false);
      ci.getPreprocessor().addPPCallbacks(
          std::make_unique<IncludeRecorder>(ci.getSourceManager(), &log_));
      // Only the rewrite pass edits, so only it records tokens.
      if (!index_pass_) {
        recorder_ = std::make_unique<TokenRecorder>(ci.getPreprocessor());
      }
      return true;
    }

    std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
        clang::CompilerInstance&, llvm::StringRef) override {
      class Consumer : public clang::ASTConsumer {
       public:
        explicit Consumer(PipelineAction& owner) : owner_(owner) {}
        void HandleTranslationUnit(clang::ASTContext& ctx) override {
          // Consume before any rewriting: the collector is complete only
          // once the whole top-level loop has run.
          const clang::syntax::TokenBuffer* tokens =
              owner_.recorder_ == nullptr ? nullptr
                                          : &owner_.recorder_->Consume();
          if (owner_.index_pass_) {
            owner_.builder_.IndexTu(ctx, &owner_.log_);
          } else {
            owner_.builder_.RewriteTreeTu(ctx, std::move(owner_.log_), tokens);
          }
        }

       private:
        PipelineAction& owner_;
      };
      return std::make_unique<Consumer>(*this);
    }

   private:
    ProgramBuilder& builder_;
    bool index_pass_;
    CollectingDiagConsumer* diags_;
    std::vector<IncludeDirective> log_;
    std::unique_ptr<TokenRecorder> recorder_;
  };

  std::vector<std::string> main_files;
  for (const VirtualTu& tu : tus) main_files.push_back(CanonicalPath(tu.path));
  CollectingDiagConsumer diags;
  ProgramBuilder builder(top, SynthTarget::kXilinxHls,
                         TreeConfig{out_root, std::move(main_files)});

  const auto run_pass = [&](bool index_pass) {
    for (const VirtualTu& tu : tus) {
      clang::tooling::runToolOnCodeWithArgs(
          std::make_unique<PipelineAction>(builder, index_pass, &diags),
          tu.code, tu.own_args.empty() ? args : tu.own_args, tu.path,
          "macro_rewrite_test",
          std::make_shared<clang::PCHContainerOperations>(), virtual_files);
    }
  };
  run_pass(/*index_pass=*/true);
  bool ok = builder.MergeAndDiscover();
  if (ok) {
    run_pass(/*index_pass=*/false);
    std::string error;
    ok = builder.WriteTree(&error);
  }
  TreePipelineRun run;
  run.diags = diags.errors;
  run.json = builder.EmitJson().dump();
  if (ok) run.files = ReadTree(out_root);
  return run;
}

bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

// ── Fixtures ──────────────────────────────────────────────────────────────

// The tapa-lib loop macro's shape (TAPA_WHILE_NOT_EOT), spelled directly
// and nested in a user macro, plus a same-line neighbor. `Fifo` carries the
// `eot` interface the macro drives (the real one comes from tapa-lib).
constexpr char kLoopMacros[] = R"tapa(
#define NOT_EOT(fifo)                                                \
  for (bool valid_##fifo; !fifo.eot(valid_##fifo) || !valid_##fifo;) \
    if (valid_##fifo)
#define FOR_EACH(fifo) NOT_EOT(fifo)
#define FLUSH(fifo) fifo.flush()
)tapa";

constexpr char kFifoDecls[] = R"tapa(
struct Fifo {
  bool eot(bool& v) { return v, true; }
  void flush() {}
};
)tapa";

// Lower-level tasks: one reading a stream, one writing it (the tops wire
// streams with a producer so the stream bookkeeping is balanced).
constexpr char kLeafTask[] = R"tapa(
void Leaf(tapa::istream<float>& in, tapa::mmap<float> mem,
          unsigned long long n) {
  for (unsigned long long i = 0; i < n; ++i) mem[i] = in.read();
}
)tapa";

constexpr char kProdTask[] = R"tapa(
void Prod(tapa::ostream<float>& out, unsigned long long n) {
  for (unsigned long long i = 0; i < n; ++i) out.write(0);
}
)tapa";

// (b,c,d) A [[tapa::pipeline]] loop macro: the pragma must land immediately
// before the expansion's `if`, the invocation must be spliced out, a
// neighbor invocation on the same line must stay spelled, and the line
// count change must re-snap the following text to its original numbering.
TEST(MacroRewrite, PipelineLoopMacroSplicesAndResnaps) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kLoopMacros +
                           kFifoDecls + kLeafTask + kProdTask +
                           R"tapa(
void LoopTop(tapa::mmap<const float> a, tapa::mmap<float> c,
             unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n).invoke(Leaf, q, c, n);
}
)tapa" + R"tapa(
void LoopTask(tapa::istream<float>& q, tapa::mmap<float> c,
              unsigned long long n) {
  Fifo f;
  [[tapa::pipeline(1)]] FOR_EACH(
      f) { q.read(); } FLUSH(f);
}
)tapa";
  const TreePipelineRun run =
      RunTreePipeline({VirtualTu{"/proj/src/loop.cpp", code}}, "LoopTop",
                      TempRoot("loop"), {"-std=c++17"});
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  ASSERT_EQ(run.files.size(), 1u) << "one TU, no includes";
  const std::string& file = run.files.at("loop.cpp");
  // The invocation is replaced by its expansion with the pragma immediately
  // before the `if` the attribute scoped to; the attribute text is gone.
  EXPECT_TRUE(
      Contains(file,
               "  for ( bool valid_f ; ! f . eot ( valid_f ) || ! valid_f ; ) "
               "_Pragma(\"HLS pipeline II = 1\") if ( valid_f )\n"
               "#line 101 \"/proj/src/loop.cpp\"\n"
               " { q.read(); } FLUSH(f);"))
      << file;
  // The same-line neighbor stays spelled.
  EXPECT_FALSE(Contains(file, "f . flush ( )"));
}

// The MacroOwnedEditIsAHardError shape from MF3, now rewriting through the
// splice: a task whose BODY is spelled by a macro still gets its guard,
// its interface preamble inside the expansion, and its signature stub.
TEST(MacroRewrite, TaskBodyInAMacroRewritesThroughSplice) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + R"tapa(
#define TAPA_TASK_BODY \
  {                    \
    c[0] = q.read();   \
  }
void MacroTask(tapa::istream<float>& q, tapa::mmap<float> c,
               unsigned long long n) TAPA_TASK_BODY
)tapa" + kLeafTask + kProdTask +
                           R"tapa(
void Top(tapa::mmap<const float> a, tapa::mmap<float> c,
         unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n).invoke(MacroTask, q, c, n);
}
)tapa";
  const TreePipelineRun run =
      RunTreePipeline({VirtualTu{"/proj/src/body.cpp", code}}, "Top",
                      TempRoot("body"), {"-std=c++17"});
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  const std::string& file = run.files.at("body.cpp");
  // The guard opens in user source (before the signature, its own #line
  // re-snap) and closes after the splice; the stub is the file-spelled
  // signature.
  EXPECT_TRUE(Contains(file,
                       "#ifdef TAPA_TASK_DEF_MacroTask\n"
                       "#line 75 \"/proj/src/body.cpp\"\n"
                       "void MacroTask(tapa::istream<float>& q, "
                       "tapa::mmap<float> c,\n"
                       "               unsigned long long n) { \n"))
      << file;
  EXPECT_TRUE(Contains(file,
                       "} \n"
                       "#else\n"
                       "void MacroTask(tapa::istream<float>& q, "
                       "tapa::mmap<float> c,\n"
                       "               unsigned long long n) ;\n"
                       "#endif\n"))
      << file;
  // The body's tokens render space-joined, preamble inside the braces.
  EXPECT_TRUE(Contains(file, "c [ 0 ] = q . read ( ) ;"));
  EXPECT_FALSE(Contains(file, "TAPA_TASK_BODY\n"));
}

// A macro-owned attribute: the whole `[[tapa::...]]`, brackets included,
// lives in the expansion. Removal drops the attribute's tokens and swallows
// the enclosing `[[ ]]` because they belong to the same invocation.
TEST(MacroRewrite, MacroOwnedAttributeIsRemovedByTokens) {
  const std::string code = std::string(kTapaStubDecls) + "\n" +
                           R"tapa(
#define ATTR_LOOP(i, n) \
  [[tapa::pipeline(1)]] for (unsigned long long i = 0; i < n; ++i)
)tapa" + kLeafTask + kProdTask +
                           R"tapa(
void AttrTask(tapa::istream<float>& in, tapa::mmap<float> mem,
              unsigned long long n) {
  ATTR_LOOP(i, n) { mem[i] = in.read(); }
}
void AttrTop(tapa::mmap<const float> a, tapa::mmap<float> c,
             unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n).invoke(AttrTask, q, c, n);
}
)tapa";
  const TreePipelineRun run =
      RunTreePipeline({VirtualTu{"/proj/src/attr.cpp", code}}, "AttrTop",
                      TempRoot("attr"), {"-std=c++17"});
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  const std::string& file = run.files.at("attr.cpp");
  // The brackets went with the attribute (an empty `[[]]` would not
  // compile), and the loop keeps its braces: the loop body's `{` is user
  // source, so its pragma is an ordinary file edit.
  EXPECT_TRUE(Contains(file,
                       "for ( unsigned long long i = 0 ; i < n ; ++ i ) "
                       "{\n"
                       "#pragma HLS pipeline II = 1\n"))
      << file;
  // The brackets went with the attribute (an empty `[[]]` would not
  // compile); the task body's only `for` is the spliced one, and only the
  // inert #define keeps the attribute spelling.
  const size_t task_for = file.find("for ( unsigned long long i = 0 ");
  ASSERT_NE(task_for, std::string::npos) << file;
  EXPECT_EQ(file.find("for ( unsigned long long i = 0 ", task_for + 1),
            std::string::npos);
}

// (a) A task invocation wrapped in a user macro (the multi-file app's
// INVOKE_CONSUME shape): the graph sees through the macro, and the top's
// rewritten shell needs no splice.
TEST(MacroRewrite, InvokeMacroStaysGraphVisible) {
  const std::string code =
      std::string(kTapaStubDecls) + "\n" + kLeafTask + kProdTask + R"tapa(
#define INVOKE_LEAF(q, c, n) .invoke(Leaf, q, c, n)
void InvokeTop(tapa::mmap<const float> a, tapa::mmap<float> c,
               unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n) INVOKE_LEAF(q, c, n);
}
)tapa";
  const TreePipelineRun run =
      RunTreePipeline({VirtualTu{"/proj/src/invoke.cpp", code}}, "InvokeTop",
                      TempRoot("invoke"), {"-std=c++17"});
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  ASSERT_EQ(run.files.size(), 1u);
  // The graph carries the edge through the macro (the tapa.json payload).
  EXPECT_TRUE(Contains(run.json, "\"Leaf\""));
  // The top's body became the interface shell (a file-level replacement
  // covering the invocation), so no splice was needed: the invocation is
  // gone, only the inert #define keeps its spelling.
  const std::string& invoke_file = run.files.at("invoke.cpp");
  EXPECT_FALSE(Contains(invoke_file, "INVOKE_LEAF(q, c, n);"));
  EXPECT_TRUE(Contains(invoke_file, "#define INVOKE_LEAF"));
}

// Two replacements of one invocation that claim overlapping tokens have no
// single honest rendering: a precise error, not a compose-order guess.
// The middle-level shell replaces the whole macro-owned body while the
// lowered attribute inside it claims a token range of its own.
TEST(MacroRewrite, OverlappingReplacementsOfOneInvocationError) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + R"tapa(
#define MID_BODY(q, c, n)                        \
  { [[tapa::pipeline(1)]]                        \
    for (unsigned long long i = 0; i < n; ++i) {}\
    tapa::task().invoke(Leaf, q, c, n); }
)tapa" + kLeafTask + kProdTask +
                           R"tapa(
void MidTask(tapa::istream<float>& q, tapa::mmap<float> c,
             unsigned long long n) MID_BODY(q, c, n)
void MidTop(tapa::mmap<const float> a, tapa::mmap<float> c,
            unsigned long long n) {
  tapa::stream<float, 2> s;
  tapa::task().invoke(Prod, s, n).invoke(MidTask, s, c, n);
}
)tapa";
  const TreePipelineRun run =
      RunTreePipeline({VirtualTu{"/proj/src/overlap.cpp", code}}, "MidTop",
                      TempRoot("overlap"), {"-std=c++17"});
  ASSERT_FALSE(run.diags.empty());
  const std::string& error = run.diags.front();
  // The error carries the construct, the reason, and the user-source
  // location of the invocation.
  EXPECT_TRUE(Contains(error, "macro expansion")) << error;
  EXPECT_TRUE(Contains(error, "overlaps another replacement")) << error;
}

// (e) The shared-header tripwire through real splices: the same header
// rewritten from two TUs renders byte-identically when their expansions
// agree, and a TU-local define that changes one expansion is a hard error
// naming both TUs -- never a silent first-wins.
constexpr char kSharedHeader[] = R"tapa(
#ifndef LIMIT
#define LIMIT 8
#endif
#define PIPE_LOOP(i) [[tapa::pipeline(1)]] for (unsigned long long i = 0; i < LIMIT; ++i)
void SharedHelper(tapa::ostream<float>& out) {
  PIPE_LOOP(i) { out.write(i); }
}
)tapa";

std::string SharedHeaderTu(const std::string& name) {
  return std::string(kTapaStubDecls) +
         "\n#include \"shared.h\"\ntapa::ostream<float>& SharedOut();\n" +
         "void " + name + "() { SharedHelper(SharedOut()); }\n";
}

TEST(MacroRewrite, SharedHeaderSplicesIdenticallyAcrossTus) {
  const clang::tooling::FileContentMappings files = {
      {"/proj/src/shared.h", kSharedHeader}};
  const TreePipelineRun run = RunTreePipeline(
      {VirtualTu{"/proj/src/a.cpp", SharedHeaderTu("A")},
       VirtualTu{"/proj/src/sub/b.cpp", SharedHeaderTu("B")}},
      "A", TempRoot("shared_ok"), {"-std=c++17", "-I/proj/src"}, files);
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  // One header in the tree, spliced identically by both TUs: the attribute
  // is gone (brackets swallowed), the expansion baked LIMIT at its default.
  ASSERT_EQ(run.files.count("shared.h"), 1u);
  EXPECT_TRUE(Contains(run.files.at("shared.h"),
                       "for ( unsigned long long i = 0 ; i < 8 ; ++ i )"))
      << run.files.at("shared.h");
  EXPECT_FALSE(Contains(run.files.at("shared.h"), "PIPE_LOOP(i) {"));
}

TEST(MacroRewrite, DivergentSharedHeaderSpliceIsAHardError) {
  const clang::tooling::FileContentMappings files = {
      {"/proj/src/shared.h", kSharedHeader}};
  // TU B redefines LIMIT, so the same header's expansion -- and with it the
  // rewritten bytes -- differs between the TUs.
  const TreePipelineRun run = RunTreePipeline(
      {VirtualTu{"/proj/src/a.cpp", SharedHeaderTu("A")},
       VirtualTu{"/proj/src/sub/b.cpp",
                 SharedHeaderTu("B"),
                 {"-std=c++17", "-I/proj/src", "-DLIMIT=16"}}},
      "A", TempRoot("shared_diverge"), {"-std=c++17", "-I/proj/src"}, files);
  ASSERT_FALSE(run.diags.empty());
  const std::string& error = run.diags.front();
  EXPECT_TRUE(Contains(error, "'/proj/src/shared.h'")) << error;
  EXPECT_TRUE(Contains(error, "'/proj/src/a.cpp'")) << error;
  EXPECT_TRUE(Contains(error, "'/proj/src/sub/b.cpp'")) << error;
  EXPECT_TRUE(Contains(error, "rewrite identically")) << error;
}

}  // namespace
}  // namespace tapa::cc
