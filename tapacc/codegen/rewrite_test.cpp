// The per-decl rewrite rules of the guarded rewritten tree: attribute
// lowering, signature rewrites, interface pragmas, and the exact stub/guard
// shape — asserted on the mirrored TU the production pipeline renders, so
// what fails here fails at HLS. Byte-sensitive fixtures use the `tapa`
// raw-string delimiter, not `cpp`: clang-format maps C++ delimiters to the
// C++ language and reflows their content, and no language is mapped to
// `tapa`.

#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "llvm/ADT/StringRef.h"
#include "nlohmann/json.hpp"

#include "test_util.h"
#include "tree_pipeline.h"

namespace tapa::cc {
namespace {

using test_support::Contains;
using test_support::TempRoot;
using test_support::TreePipelineRun;
using test_support::VirtualTu;

constexpr char kVadd[] = R"cpp(
  void Mmap2Stream(tapa::mmap<const float> mem, unsigned long long n,
                   tapa::ostream<float>& out) {
    for (unsigned long long i = 0; i < n; ++i) out.write(mem[i]);
  }
  void Add(tapa::istream<float>& a, tapa::istream<float>& b,
           tapa::ostream<float>& c, unsigned long long n) {
    for (unsigned long long i = 0; i < n; ++i) c.write(a.read() + b.read());
  }
  void Stream2Mmap(tapa::istream<float>& in, tapa::mmap<float> mem,
                   unsigned long long n) {
    for (unsigned long long i = 0; i < n; ++i) mem[i] = in.read();
  }
  void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
              tapa::mmap<float> c, unsigned long long n) {
    tapa::stream<float, 8> a_q;
    tapa::stream<float, 8> b_q;
    tapa::stream<float, 8> c_q;
    tapa::task()
        .invoke(Mmap2Stream, a, n, a_q)
        .invoke(Mmap2Stream, b, n, b_q)
        .invoke(Add, a_q, b_q, c_q, n)
        .invoke(Stream2Mmap, c_q, c, n);
  }
  void UnusedTask(tapa::mmap<const float> a, tapa::mmap<const float> b,
                  tapa::mmap<float> c, unsigned long long n) {
    tapa::stream<float, 8> a_q;
    tapa::stream<float, 8> b_q;
    tapa::stream<float, 8> c_q;
    tapa::task()
        .invoke(Mmap2Stream, a, n, a_q)
        .invoke(Mmap2Stream, b, n, b_q)
        .invoke(Add, a_q, b_q, c_q, n)
        .invoke(Stream2Mmap, c_q, c, n);
  }
)cpp";

// One clean single-TU run over `code`, top `top`: the mirrored TU the way
// tapacc writes it.
std::string Rewrite(const std::string& name, const std::string& code,
                    const std::string& top) {
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/" + name + ".cpp", code}}, top,
      TempRoot("rewrite", name), {"-std=c++17"});
  EXPECT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  EXPECT_EQ(run.files.size(), 1u) << "one TU, no includes";
  return run.files.at(name + ".cpp");
}

std::string RewriteVadd() { return Rewrite("vadd", kVadd, "VecAdd"); }

TEST(Rewrite, VitisStreamsUseByteAlignedPayloadForSignatureAndWrites) {
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/stream.cpp", R"cpp(
#include "stream.h"
                   void Copy(tapa::istream<Word>& in, tapa::ostream<Word>& out,
                             tapa::ostream<tapa::i<17>>& narrow) {
                     out.write(in.read());
                   }
                   void Top(tapa::istream<Word>& in, tapa::ostream<Word>& out,
                            tapa::ostream<tapa::i<17>>& narrow) {
                     tapa::task().invoke(Copy, in, out, narrow);
                   }
                 )cpp"}},
      "Top", TempRoot("rewrite", "stream"), {"-std=c++17"},
      {{"/proj/src/stream.h", R"cpp(
          namespace tapa {
          template <int W>
          struct u {
            char storage[128];
          };
          template <int W>
          struct i {
            char storage[128];
          };
          }  // namespace tapa
          using Word = tapa::u<544>;
          void Top(tapa::istream<Word>& in, tapa::ostream<Word>& out,
                   tapa::ostream<tapa::i<17>>& narrow);
        )cpp"}},
      {{"Top", SynthTarget::kXilinxVitis}});
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  const auto& code = run.files.at("stream.cpp");
  const auto& header = run.files.at("stream.h");
  EXPECT_TRUE(Contains(header, "hls::stream<qdma_axis<544, 0, 0, 0> >& in"));
  EXPECT_TRUE(Contains(header, "hls::stream<qdma_axis<24, 0, 0, 0> >& narrow"));
  for (const auto* file : {&code, &header}) {
    // Vendor templates must be declared before the rewritten signature and
    // outside its C linkage block, including for declarations in headers.
    EXPECT_LT(file->find("#include \"ap_axi_sdata.h\""),
              file->find("extern \"C\""));
    EXPECT_LT(file->find("#include \"hls_stream.h\""),
              file->find("extern \"C\""));
  }
  EXPECT_TRUE(Contains(code, "hls::stream<qdma_axis<544, 0, 0, 0> >& in"));
  EXPECT_TRUE(Contains(code, "hls::stream<qdma_axis<544, 0, 0, 0> >& out"));
  EXPECT_TRUE(Contains(code, "hls::stream<qdma_axis<24, 0, 0, 0> >& narrow"));
  EXPECT_TRUE(Contains(code, "out.write(qdma_axis<544, 0, 0, 0>());"));
  EXPECT_TRUE(Contains(code, "narrow.write(qdma_axis<24, 0, 0, 0>());"));
}

// A Vitis top's stream argument that reaches a frame port, here through an
// upper task, moves through a ring in device memory: the argument becomes
// the ring's base address, and the ring's registers follow the top's last
// parameter so the host's argument indices stay the parameters'.
TEST(Rewrite, TopFrameArgumentsBecomeRings) {
  constexpr char kRing[] = R"cpp(
    void Turn([[tapa::frame(64)]] tapa::istream<float>& in,
              [[tapa::frame(64)]] tapa::ostream<float>& out, int n) {
      for (int i = 0; i < n * 64; ++i) out.write(in.read());
    }
    void Mid(tapa::istream<float>& in, tapa::ostream<float>& out, int n) {
      tapa::task().invoke(Turn, in, out, n);
    }
    void Top(tapa::istream<float>& rows,
             [[tapa::ring(8)]] tapa::ostream<float>& cols, int n) {
      tapa::task().invoke(Mid, rows, cols, n);
    }
  )cpp";
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/ring.cpp", kRing}}, "Top",
      TempRoot("rewrite", "ring"), {"-std=c++17"}, {},
      {{"Top", SynthTarget::kXilinxVitis}});
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  const nlohmann::json ports =
      nlohmann::json::parse(run.json)["tasks"]["Top"]["ports"];
  EXPECT_EQ(ports[0]["ring"], 4);
  EXPECT_EQ(ports[1]["ring"], 8);
  EXPECT_FALSE(ports[2].contains("ring"));
  const std::string& code = run.files.at("ring.cpp");
  // The rewrite keeps the source's line break between the parameters.
  EXPECT_TRUE(Contains(code, "void Top(uint64_t rows_offset,\n")) << code;
  EXPECT_TRUE(
      Contains(code,
               "uint64_t cols_offset, int n, "
               "uint32_t rows_host_frames, uint32_t& rows_kernel_frames, "
               "uint64_t& rows_ring_shape, uint32_t cols_host_frames, "
               "uint32_t& cols_kernel_frames, uint64_t& cols_ring_shape)"))
      << code;
  EXPECT_TRUE(Contains(
      code,
      "HLS interface s_axilite port = cols_kernel_frames bundle = control"))
      << code;
  EXPECT_FALSE(Contains(code, "qdma_axis")) << code;
  EXPECT_FALSE(Contains(code, "tapa::ring")) << code;

  // Only such an argument has a ring.
  constexpr char kStray[] = R"cpp(
    void Copy(tapa::istream<float>& in, tapa::ostream<float>& out) {
      out.write(in.read());
    }
    void Top([[tapa::ring(8)]] tapa::istream<float>& in,
             tapa::ostream<float>& out) {
      tapa::task().invoke(Copy, in, out);
    }
  )cpp";
  const TreePipelineRun stray = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/stray.cpp", kStray}}, "Top",
      TempRoot("rewrite", "stray"), {"-std=c++17"}, {},
      {{"Top", SynthTarget::kXilinxVitis}});
  EXPECT_FALSE(stray.ok);
  bool reported = false;
  for (const std::string& error : stray.builder.errors()) {
    reported |= Contains(error,
                         "only a stream argument of the top task that "
                         "reaches a frame port has a ring");
  }
  EXPECT_TRUE(reported);
}

constexpr char kAttrs[] = R"cpp(
  float DeclFirst(int);
  inline float DeclFirst(int x) { return x + 1; }
  inline int Second(int);
  int Second(int x) { return x + 2; }
  // Function-level pipeline lowers into the body it is written on.
  [[tapa::pipeline(2, "flp")]] float FnPipelined(float x) { return x + 8; }
  inline float Scale(float x) { return x * 2; }
  float Mix(float a, float b) { return a + b; }
  static float StaticHelper(float x) { return x + 3; }
  // Shares a name with the top task: an overload, and still a helper.
  static float Top(float x) { return x + 6; }
  static inline float StaticInlineHelper(float x) { return x + 4; }
  namespace {
  float AnonHelper(float x) { return x + 5; }
  }  // namespace
  template <typename T>
  inline T Twice(T x) {
    return x + x;
  }
  void AttrTask(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                unsigned long long n) {
    float buf[64];
    [[tapa::partition("cyclic", 32)]] float a[32];
    [[tapa::partition("complete", -1, 0)]] float stencil[3][3];  // dim 0 = all
    [[tapa::storage("RAM_2P", "URAM")]] float local_c[128];
    [[tapa::aggregate]] float tmpv[4];
    [[tapa::bind_op("add", "dsp")]] float acc = 0;
    [[tapa::array_map("local_A", 128, "horizontal")]] float local_A_ping[64];
    [[tapa::array_map("local_B")]] float local_B_pong[64];
    // One spelling announcing two declarators: one pragma each, one removal.
    [[tapa::aggregate]] float x0[8], x1[8];
    [[tapa::tripcount(1, 800)]] for (unsigned long long i = 0; i < n; ++i)
      out.write(mem[i]);
    [[tapa::flatten]] for (int i = 0; i < 4; ++i)
      buf[i] = 0;
    [[tapa::latency(2, 2)]] for (int i = 0; i < 8; ++i)
      buf[i] += 1;
    [[tapa::latency(0, 0)]] for (int i = 0; i < 8; ++i)
      buf[i] -= 1;
    [[tapa::flatten(false)]] for (int i = 0; i < 4; ++i)
      buf[i] *= 3;
    [[tapa::dependence("v", "", "inter", "RAW", 1, 6)]] for (int i = 0; i < 4;
                                                             ++i)
      buf[i] += 4;
    [[tapa::dependence("w", "", "intra", "", 1)]] for (int i = 0; i < 4; ++i)
      buf[i] += 5;
    [[tapa::dependence("local_c", "", "inter")]]
    for (int i = 0; i < 8; ++i)
      local_c[i] = buf[i];
    [[tapa::balance]] for (int i = 0; i < 8; ++i)
      buf[i] *= 2;
    // 0 disables pipelining, as flatten(false) disables flattening; the
    // bare spelling still asks for the vendor's default interval.
    [[tapa::pipeline(false)]] for (int i = 0; i < 4; ++i)
      buf[i] -= 2;
    [[tapa::pipeline]] for (int i = 0; i < 4; ++i)
      buf[i] += 7;
    // A region attribute on an `if`: the vendor pragma it migrates from sat
    // inside the braces, so it has to lower there too.
    [[tapa::latency(1, 1)]] if (n > 0) {
      buf[0] += 1;
      buf[1] += 2;
    }
  }
  void Top(tapa::mmap<const float> mem, tapa::ostream<float>& out,
           unsigned long long n) {
    tapa::task().invoke(AttrTask, mem, out, n);
  }
)cpp";

TEST(Rewrite, StmtAttrsLowerToPragmas) {
  const std::string code = Rewrite("attrs", kAttrs, "Top");
  // An `if` region takes the pragma INSIDE its braces: before the `if`
  // would hand the constraint to the enclosing region instead. The
  // pragma's synthetic lead line sits between the `{` and the pragma.
  ASSERT_NE(code.find("if (n > 0) {"), std::string::npos);
  EXPECT_GT(code.find("#pragma HLS latency min = 1 max = 1"),
            code.find("if (n > 0) {"));
  EXPECT_TRUE(Contains(code, "HLS pipeline off"));
  // The function-level form lowers into the body; leaving the raw
  // attribute would hand `[[tapa::pipeline]]` to the vendor compiler.
  EXPECT_TRUE(Contains(code, "HLS pipeline style = flp II = 2"));
  EXPECT_FALSE(Contains(code, "tapa::pipeline"));
  // Bare `[[tapa::pipeline]]` still means "pipeline, vendor's II", not off.
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS pipeline\")"));
  // Single-statement loop bodies lower to _Pragma, compound bodies to
  // #pragma (same as pipeline/unroll).
  EXPECT_TRUE(
      Contains(code, "_Pragma(\"HLS loop_tripcount min = 1 max = 800\")"));
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS loop_flatten\")"));
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS latency min = 2 max = 2\")"));
  // Zero bounds are meaningful (combinational), not "absent".
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS latency min = 0 max = 0\")"));
  // Flatten-off and the vendor dependence-true forms round-trip.
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS loop_flatten off\")"));
  EXPECT_TRUE(Contains(code,
                       "_Pragma(\"HLS dependence variable = v type = inter "
                       "direction = RAW dependent = true distance = 6\")"));
  EXPECT_TRUE(Contains(code,
                       "_Pragma(\"HLS dependence variable = w type = intra "
                       "dependent = true\")"));
  EXPECT_TRUE(Contains(code,
                       "_Pragma(\"HLS dependence variable = local_c type = "
                       "inter dependent = false\")"));
  EXPECT_TRUE(Contains(code, "_Pragma(\"HLS expression_balance\")"));
  // The attribute spellings themselves are removed from the source.
  EXPECT_FALSE(Contains(code, "tapa::tripcount"));
  EXPECT_FALSE(Contains(code, "tapa::dependence"));
}

TEST(Rewrite, DeclAttrsAndInlineRuleLowerToPragmas) {
  const std::string code = Rewrite("attrs", kAttrs, "Top");
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS array_partition variable = a type = cyclic "
                       "factor = 32"));
  // A dim-only partition: the -1 factor sentinel is omitted from the
  // pragma, and the dim survives. Emitting `factor = ...` here instead
  // would silently partition dimension 1.
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS array_partition variable = stencil type = "
                       "complete dim = 0"));
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS bind_storage variable = local_c type = "
                       "RAM_2P impl = URAM"));
  EXPECT_TRUE(Contains(code, "#pragma HLS aggregate variable = tmpv"));
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS bind_op variable = acc op = add impl = "
                       "dsp"));
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS array_map variable = local_A_ping instance "
                       "= local_A offset = 128 horizontal"));
  // Minimal form: only the instance name; offset/orient keywords omitted.
  EXPECT_TRUE(Contains(code,
                       "#pragma HLS array_map variable = local_B_pong instance "
                       "= local_B\n"));
  EXPECT_TRUE(Contains(code, "#pragma HLS aggregate variable = x0"));
  EXPECT_TRUE(Contains(code, "#pragma HLS aggregate variable = x1"));
  EXPECT_FALSE(Contains(code, "tapa::partition"));
  EXPECT_FALSE(Contains(code, "tapa::storage"));
  // `inline` present -> always_inline + inline pragma.
  EXPECT_TRUE(
      Contains(code, "__attribute__((always_inline)) inline float Scale"));
  // Redeclaration chains: the definition is rewritten once with the chain's
  // policy; `inline` on any declaration means inline (the keyword is added
  // to the definition for always_inline's legality).
  EXPECT_TRUE(
      Contains(code, " __attribute__((always_inline)) inline float DeclFirst"));
  EXPECT_TRUE(
      Contains(code, " __attribute__((always_inline)) inline int Second"));
  EXPECT_TRUE(Contains(code, "#pragma HLS inline\n"));
  // No keyword -> never inline.
  EXPECT_TRUE(Contains(code, "__attribute__((noinline)) float Mix"));
  // Internal linkage is not an exemption from the inline policy: without a
  // control the vendor picks the hierarchy itself.
  EXPECT_TRUE(
      Contains(code, "__attribute__((noinline)) static float StaticHelper"));
  EXPECT_TRUE(Contains(code,
                       "__attribute__((always_inline)) static inline float "
                       "StaticInlineHelper"));
  EXPECT_TRUE(Contains(code, "__attribute__((noinline)) float AnonHelper"));
  // Sharing a name with a task is not a reason to skip a helper: tasks are
  // discovered from the global-function list, so this overload cannot be one.
  EXPECT_TRUE(Contains(code, "__attribute__((noinline)) static float Top"));
  EXPECT_TRUE(Contains(code, "#pragma HLS inline off\n"));
  // Template helper: the attribute follows the template header (and the
  // `inline` keyword may stay on its own line).
  EXPECT_TRUE(Contains(code, "> __attribute__((always_inline))"));
  EXPECT_TRUE(Contains(code, "inline T Twice"));
}

TEST(Rewrite, OutOfRangePositionalIntIsRejected) {
  // -1 is the "omitted" sentinel for the positional factor/dim pair; any
  // other negative must be rejected, not zero-extended into the uint32 and
  // emitted verbatim (`factor = -7`).
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/oor.cpp", R"cpp(
                   void Task(tapa::ostream<float>& out) {
                     [[tapa::partition("cyclic", -7)]] float a[32];
                     out.write(a[0]);
                   }
                   void Top(tapa::ostream<float>& out) { tapa::task().invoke(Task, out); }
                 )cpp"}},
      "Top", TempRoot("rewrite", "oor"), {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  EXPECT_FALSE(run.diags.empty()) << "a factor below -1 must be diagnosed";
}

// A declaration has no body to lower a function-level pipeline into;
// the rewrite rejects it where it is written instead of letting the raw
// text reach the vendor.
TEST(Rewrite, PipelineOnDeclarationIsRejected) {
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/pipedecl.cpp", R"cpp(
                   [[tapa::pipeline(2)]] void Task(tapa::ostream<float>& out);
                   void Task(tapa::ostream<float>& out) { out.write(1.f); }
                   void Top(tapa::ostream<float>& out) { tapa::task().invoke(Task, out); }
                 )cpp"}},
      "Top", TempRoot("rewrite", "pipedecl"), {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_EQ(run.diags.size(), 1u) << run.json;
  EXPECT_TRUE(Contains(run.diags.front(), "on a function declaration"))
      << run.diags.front();
}

TEST(Rewrite, LowerTaskGetsFifoPragmas) {
  const std::string code = RewriteVadd();

  // istream ports get ap_fifo interface + peek pragmas and empty() stubs.
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_fifo port = a._"));
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_fifo port = a._peek"));
  EXPECT_TRUE(Contains(code, "void(a._.empty());"));
  // ostream port gets a full() stub.
  EXPECT_TRUE(Contains(code, "void(c._.full());"));
  // The pipeline-free loop body stays; the task keeps its computation.
  EXPECT_TRUE(Contains(code, "a.read()"));
}

TEST(Rewrite, EmitsGuardsAndExactRewrittenStubs) {
  const std::string code = RewriteVadd();

  EXPECT_TRUE(Contains(code, "#ifdef TAPA_TASK_DEF_Add"));
  EXPECT_TRUE(Contains(code,
                       "#else\n"
                       "void Add(tapa::istream<float>& a, "
                       "tapa::istream<float>& b,"))
      << code;
  EXPECT_TRUE(Contains(code, "#ifdef TAPA_TASK_DEF_VecAdd"));
  // The else-branch stub drops the mmap typedef spelling for the offset
  // form.
  EXPECT_FALSE(Contains(code, "void VecAdd(tapa::mmap<const float> a"));

  // The upper task's mmap-to-offset rewrite is byte-identical in the full
  // definition and the else-branch declaration: one signature spelling,
  // exactly two occurrences.
  constexpr llvm::StringLiteral kTopSignature(
      "void VecAdd(uint64_t a_offset, uint64_t b_offset,");
  size_t count = 0;
  for (size_t pos = code.find(kTopSignature); pos != std::string::npos;
       pos = code.find(kTopSignature, pos + kTopSignature.size())) {
    ++count;
  }
  EXPECT_EQ(count, 2u) << code;
}

TEST(Rewrite, UpperTaskBecomesShellWithOffsets) {
  const std::string code = RewriteVadd();

  // mmap parameters are lowered to uint64 offsets in the signature.
  EXPECT_TRUE(Contains(code, "uint64_t a_offset"));
  // The body is replaced by an interface shell (no task() / invoke left).
  EXPECT_FALSE(Contains(code, ".invoke("));
  // Middle-level scalar/offset ports get ap_none register pragmas.
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_none port = a_offset"));
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_none port = n"));
}

TEST(Rewrite, UnreachableTaskIsStrippedLikeOtherTasks) {
  const std::string code = RewriteVadd();

  // UnusedTask is unreachable from the top, but its body still invokes
  // sub-tasks with mmap args; it must be stripped to its rewritten
  // signature or the uint64_t sub-task signatures no longer type-check.
  EXPECT_TRUE(Contains(code, "UnusedTask(uint64_t a_offset"));
  EXPECT_FALSE(Contains(code, ".invoke("));
}

// An attribute Clang itself drops: [[tapa::partition]] applies to variables,
// so on a function it never reaches the AST -- and no removal pass can see
// what is not there. Its text stays in the mirror the tree writer renders,
// and the vendor, for which an unknown attribute is not an error, ignores a
// directive the user wrote. The emitted tree is checked for exactly this.
constexpr char kLeakedAttr[] = R"cpp(
  [[tapa::partition("complete")]] void Helper(float a[4]) {
    a[0] = 1;
  }
  void LeakTask(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                unsigned long long n) {
    float buf[4];
    Helper(buf);
    for (unsigned long long i = 0; i < n; ++i) out.write(mem[i]);
  }
  void LeakTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
               unsigned long long n) {
    tapa::task().invoke(LeakTask, mem, out, n);
  }
)cpp";

// Run twice in one process: each run reports its own leaks, so the second
// must not be silenced by what the first already reported.
TEST(Rewrite, AttrThatCannotLowerIsAnError) {
  for (int pass = 0; pass < 2; ++pass) {
    const TreePipelineRun run = test_support::RunTreePipeline(
        {VirtualTu{"/proj/src/leak.cpp", kLeakedAttr}}, "LeakTop",
        TempRoot("rewrite", "leak"), {"-std=c++17"});
    EXPECT_FALSE(run.ok) << "pass " << pass;
    ASSERT_EQ(run.diags.size(), 1u) << "pass " << pass;
    EXPECT_TRUE(Contains(run.diags.front(), "[[tapa::partition]]"))
        << run.diags.front();
  }
}

// `[[tapa::target]]` takes the values `--target TASK:TARGET` does; `hls` builds
// the task in the design's flow, and a retired flow value is an error.
TEST(Rewrite, TargetTakesTheCliMappingValues) {
  constexpr char kHls[] = R"cpp(
    [[tapa::target("hls")]] void Copy(tapa::istream<float>& in,
                                      tapa::ostream<float>& out) {
      out.write(in.read());
    }
    void CopyTop(tapa::istream<float>& in, tapa::ostream<float>& out) {
      tapa::task().invoke(Copy, in, out);
    }
  )cpp";
  const TreePipelineRun hls = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/hls.cpp", kHls}}, "CopyTop",
      TempRoot("rewrite", "target_hls"), {"-std=c++17"});
  EXPECT_TRUE(hls.ok) << (hls.diags.empty() ? hls.json : hls.diags.front());

  const std::string retired = std::string(kHls).replace(
      std::string(kHls).find("\"hls\""), 5, "\"xilinx_vitis\"");
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/retired_target.cpp", retired}}, "CopyTop",
      TempRoot("rewrite", "target_retired"), {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_FALSE(run.diags.empty());
  EXPECT_TRUE(Contains(run.diags.front(),
                       "unknown TAPA target 'xilinx_vitis'; use hls, aie or "
                       "ignore"))
      << run.diags.front();
}

// A misspelled or retired TAPA attribute is an error even through a macro,
// whose body the leaked-attribute scan leaves alone.
TEST(Rewrite, UnknownTapaAttributeIsAnError) {
  constexpr char kRetired[] = R"cpp(
#define FRAME [[tapa::aie_frame(4)]]
    void Copy(FRAME tapa::istream<float>& in, tapa::ostream<float>& out) {
      out.write(in.read());
    }
    void CopyTop(tapa::istream<float>& in, tapa::ostream<float>& out) {
      tapa::task().invoke(Copy, in, out);
    }
  )cpp";
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/retired.cpp", kRetired}}, "CopyTop",
      TempRoot("rewrite", "retired"), {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_FALSE(run.diags.empty());
  EXPECT_TRUE(Contains(run.diags.front(),
                       "unknown TAPA attribute '[[tapa::aie_frame]]'"))
      << run.diags.front();
}

// A comment or string literal QUOTING an attribute spelling is not a leaked
// attribute: the guard would otherwise fail a kernel for prose.
TEST(Rewrite, AttrSpellingInCommentIsNotALeak) {
  constexpr char kCommentedAttr[] = R"cpp(
    // TODO: this loop wants [[tapa::pipeline]], not a pragma.
    const char* doc = "[[tapa::unroll(2)]] also works";
    /* and a block comment naming [[tapa::flatten]] */
    void QuotedTask(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                    unsigned long long n) {
      for (unsigned long long i = 0; i < n; ++i) out.write(mem[i]);
    }
    void QuotedTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                   unsigned long long n) {
      tapa::task().invoke(QuotedTask, mem, out, n);
    }
  )cpp";
  const std::string code = Rewrite("quoted", kCommentedAttr, "QuotedTop");
  EXPECT_TRUE(Contains(code, "[[tapa::pipeline")) << code;
}

// tapacc never parses a branch the preprocessor skips, so it cannot lower an
// attribute there; it removes it rather than hand it to a vendor that
// compiles the branch (chess defines TAPA_TARGET_AIE_, tapacc does not).
TEST(Rewrite, AttrInSkippedCodeIsRemoved) {
  constexpr char kGuarded[] = R"cpp(
    void GuardedTask(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                     unsigned long long n) {
#if defined(TAPA_TARGET_AIE_)
      [[tapa::pipeline(1)]] for (unsigned long long i = 0; i < n; ++i)
        out.write(mem[i] * 2);  // keeps [[tapa::unroll]] in a comment
#else
      [[tapa::pipeline(1)]] for (unsigned long long i = 0; i < n; ++i)
        out.write(mem[i]);
#endif
    }
    void GuardedTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                    unsigned long long n) {
      tapa::task().invoke(GuardedTask, mem, out, n);
    }
  )cpp";
  const std::string code = Rewrite("guarded", kGuarded, "GuardedTop");
  EXPECT_TRUE(Contains(code, "HLS pipeline II = 1")) << code;
  EXPECT_FALSE(Contains(code, "[[tapa::pipeline")) << code;
  EXPECT_TRUE(Contains(code, "keeps [[tapa::unroll]] in a comment")) << code;
}

// A forward declaration shares the definition's body through the
// redeclaration chain, so rewriting it too would insert every stream
// pragma a second time at the same point.
TEST(Rewrite, ForwardDeclaredHelperGetsOneStreamPragma) {
  constexpr char kFwdHelper[] = R"cpp(
    float FwdHelper(int x);
    float FwdHelper(int x) {
      tapa::stream<float, 4> s;
      s.write(1.0f);
      return s.read() + x;
    }
    void FwdTask(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                 unsigned long long n) {
      for (unsigned long long i = 0; i < n; ++i) out.write(FwdHelper(mem[i]));
    }
    void FwdTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                unsigned long long n) {
      tapa::task().invoke(FwdTask, mem, out, n);
    }
  )cpp";
  const std::string code = Rewrite("fwd", kFwdHelper, "FwdTop");

  const std::string pragma = "HLS stream variable = s";
  size_t count = 0;
  for (size_t pos = code.find(pragma); pos != std::string::npos;
       pos = code.find(pragma, pos + pragma.size())) {
    ++count;
  }
  EXPECT_EQ(count, 1u) << code;
}

// Declaration-only TUs close the wrapper at the semicolon, without
// consuming either an adjacent declaration or a following blank line.
TEST(Rewrite, VitisDeclarationOnlyTuClosesExternCAtSemicolon) {
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/def.cpp",
                 "void Top(tapa::mmap<const float> mem) { tapa::task(); }\n"},
       VirtualTu{"/proj/src/adjacent.cpp",
                 "void Top(tapa::mmap<const float> mem);\n"
                 "int Next();\n"},
       VirtualTu{"/proj/src/blank.cpp",
                 "void Top(tapa::mmap<const float> mem);\n\n"
                 "int Next();\n"}},
      "Top", TempRoot("rewrite", "decl_only_extern_c"), {"-std=c++17"}, {},
      {{"Top", SynthTarget::kXilinxVitis}});
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  const auto suffix = [](const std::string& code) {
    const size_t begin = code.find("extern \"C\" {");
    EXPECT_NE(begin, std::string::npos) << code;
    return begin == std::string::npos ? std::string() : code.substr(begin);
  };
  EXPECT_EQ(suffix(run.files.at("adjacent.cpp")),
            R"tapa(extern "C" {


#line 76 "/proj/src/adjacent.cpp"
void Top(uint64_t mem_offset);
#line 76 "tapa:/proj/src/adjacent.cpp"


}  // extern "C"


#line 77 "/proj/src/adjacent.cpp"
int Next();
)tapa");
  EXPECT_EQ(suffix(run.files.at("blank.cpp")),
            R"tapa(extern "C" {


#line 76 "/proj/src/blank.cpp"
void Top(uint64_t mem_offset);
#line 76 "tapa:/proj/src/blank.cpp"


}  // extern "C"


#line 77 "/proj/src/blank.cpp"

int Next();
)tapa");
}

// `[[tapa::target]]` picks the backend, which discovery has already acted
// on, so the mirror drops its text like any consumed attribute, and it is
// never reported as a leak. Left in place it would precede the
// `extern "C"` a Vitis top's definition gains, which does not parse.
constexpr char kTargetAttr[] = R"cpp(
  [[tapa::target("ignore")]] void TargetTask(tapa::mmap<const float> mem,
                                             tapa::ostream<float>& out,
                                             unsigned long long n) {
    for (unsigned long long i = 0; i < n; ++i) out.write(mem[i]);
  }
  void TargetTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                 unsigned long long n) {
    tapa::task().invoke(TargetTask, mem, out, n);
  }
)cpp";

TEST(Rewrite, TargetAttrIsDroppedOnceConsumed) {
  const std::string code = Rewrite("target", kTargetAttr, "TargetTop");
  ASSERT_NE(code.find("#ifdef TAPA_TASK_DEF_TargetTask"), std::string::npos)
      << code;
  EXPECT_FALSE(Contains(code, "[[tapa::target")) << code;
  EXPECT_FALSE(Contains(code, "[[#ifdef")) << code;
}

}  // namespace
}  // namespace tapa::cc
