// Production-path tests for selective macro expansion: the
// full pipeline -- index pass, merge, rewrite pass with the token stream
// recorded, guarded mirror materialization -- over constructs a rewrite
// anchors inside a macro expansion. Each fixture asserts the exact rendered
// bytes, so an edit silently landing outside the spliced expansion fails
// here rather than at HLS. The mechanics behind these paths are pinned in
// expansion_splice_test.cpp. The pipeline harness itself is
// codegen/tree_pipeline.h.
//
// The fixtures use the `tapa` raw-string delimiter, not `cpp`:
// clang-format maps C++ delimiters to the C++ language and reflows their
// content, which the byte-exact assertions cannot tolerate. No language is
// mapped to `tapa`, so they stay byte-stable under formatting.

#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "tree_pipeline.h"

namespace tapa::cc {
namespace {

// ── Test scaffolding ──────────────────────────────────────────────────────

// One writable directory per test, under the gtest temp root.
std::string TempRoot(const std::string& name) {
  return testing::TempDir() + "/macro_rewrite_" + name;
}

using test_support::TreePipelineRun;
using test_support::VirtualTu;

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
  const std::string code = std::string(kLoopMacros) + kFifoDecls + kLeafTask +
                           kProdTask +
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
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/loop.cpp", code}}, "LoopTop", TempRoot("loop"),
      {"-std=c++17"});
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

// A task body spelled by a macro, rewriting through the
// splice: a task whose BODY is spelled by a macro still gets its guard,
// its interface preamble inside the expansion, and its signature stub.
TEST(MacroRewrite, TaskBodyInAMacroRewritesThroughSplice) {
  const std::string code = std::string(R"tapa(
#define TAPA_TASK_BODY \
  {                    \
    c[0] = q.read();   \
  }
void MacroTask(tapa::istream<float>& q, tapa::mmap<float> c,
               unsigned long long n) TAPA_TASK_BODY
)tapa") + kLeafTask + kProdTask +
                           R"tapa(
void Top(tapa::mmap<const float> a, tapa::mmap<float> c,
         unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n).invoke(MacroTask, q, c, n);
}
)tapa";
  const TreePipelineRun run =
      test_support::RunTreePipeline({VirtualTu{"/proj/src/body.cpp", code}},
                                    "Top", TempRoot("body"), {"-std=c++17"});
  ASSERT_TRUE(run.diags.empty()) << run.diags.front();
  const std::string& file = run.files.at("body.cpp");
  // The guard opens in user source (before the signature, its own #line
  // re-snap) and closes after the splice; the stub is the file-spelled
  // signature.
  EXPECT_TRUE(Contains(file,
                       "#ifdef TAPA_TASK_DEF_MacroTask\n"
                       "\n"
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
  const std::string code = std::string(R"tapa(
#define ATTR_LOOP(i, n) \
  [[tapa::pipeline(1)]] for (unsigned long long i = 0; i < n; ++i)
)tapa") + kLeafTask + kProdTask +
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
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/attr.cpp", code}}, "AttrTop", TempRoot("attr"),
      {"-std=c++17"});
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
  const std::string code = std::string(kLeafTask) + kProdTask + R"tapa(
#define INVOKE_LEAF(q, c, n) .invoke(Leaf, q, c, n)
void InvokeTop(tapa::mmap<const float> a, tapa::mmap<float> c,
               unsigned long long n) {
  tapa::stream<float, 2> q;
  tapa::task().invoke(Prod, q, n) INVOKE_LEAF(q, c, n);
}
)tapa";
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/invoke.cpp", code}}, "InvokeTop",
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
  const std::string code = std::string(R"tapa(
#define MID_BODY(q, c, n)                        \
  { [[tapa::pipeline(1)]]                        \
    for (unsigned long long i = 0; i < n; ++i) {}\
    tapa::task().invoke(Leaf, q, c, n); }
)tapa") + kLeafTask + kProdTask +
                           R"tapa(
void MidTask(tapa::istream<float>& q, tapa::mmap<float> c,
             unsigned long long n) MID_BODY(q, c, n)
void MidTop(tapa::mmap<const float> a, tapa::mmap<float> c,
            unsigned long long n) {
  tapa::stream<float, 2> s;
  tapa::task().invoke(Prod, s, n).invoke(MidTask, s, c, n);
}
)tapa";
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/overlap.cpp", code}}, "MidTop", TempRoot("overlap"),
      {"-std=c++17"});
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
  return "#include \"shared.h\"\ntapa::ostream<float>& SharedOut();\nvoid " +
         name + "() { SharedHelper(SharedOut()); }\n";
}

TEST(MacroRewrite, SharedHeaderSplicesIdenticallyAcrossTus) {
  const clang::tooling::FileContentMappings files = {
      {"/proj/src/shared.h", kSharedHeader}};
  const TreePipelineRun run = test_support::RunTreePipeline(
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
  const TreePipelineRun run = test_support::RunTreePipeline(
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
