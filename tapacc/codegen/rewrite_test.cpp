#include "rewrite.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "frontend/build_program.h"
#include "frontend/program.h"
#include "frontend/tapa_stub_decls.h"
#include "xilinx.h"

namespace tapa::cc {
namespace {

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

struct Emitted {
  std::unique_ptr<clang::ASTUnit> ast;
  Program program;
};

Emitted Build() {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kVadd;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "VecAdd", SynthTarget::kXilinxHls);
  return Emitted{std::move(ast), std::move(program)};
}

bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

constexpr char kAttrs[] = R"cpp(
  float DeclFirst(int);
  inline float DeclFirst(int x) { return x + 1; }
  inline int Second(int);
  int Second(int x) { return x + 2; }
  // Function-level pipeline: accepted on declarations for legacy
  // compatibility, so it must lower rather than leak its text.
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
    tapa::stream<float, 2> q;
    tapa::task().invoke(AttrTask, mem, q, n);
  }
)cpp";

struct AttrEmitted {
  std::unique_ptr<clang::ASTUnit> ast;
  Program program;
};

struct PrintingDiagConsumer : clang::DiagnosticConsumer {
  void HandleDiagnostic(clang::DiagnosticsEngine::Level,
                        const clang::Diagnostic& info) override {
    llvm::SmallVector<char, 128> msg;
    info.FormatDiagnostic(msg);
    llvm::errs() << "diag: " << llvm::StringRef(msg.data(), msg.size()) << "\n";
  }
};

AttrEmitted BuildAttrs() {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kAttrs;
  PrintingDiagConsumer diag;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), &diag);
  EXPECT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "Top", SynthTarget::kXilinxHls);
  return AttrEmitted{std::move(ast), std::move(program)};
}

TEST(Rewrite, StmtAttrsLowerToPragmas) {
  auto e = BuildAttrs();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code =
      EmitTaskCode(e.program, e.program.tasks.at("AttrTask"), backend,
                   e.ast->getASTContext());
  // An `if` region takes the pragma INSIDE its braces: before the `if`
  // would hand the constraint to the enclosing region instead.
  EXPECT_TRUE(
      Contains(code, "if (n > 0) {\n#pragma HLS latency min = 1 max = 1"));
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
  auto e = BuildAttrs();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code =
      EmitTaskCode(e.program, e.program.tasks.at("AttrTask"), backend,
                   e.ast->getASTContext());
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

// Counts errors so a rejected attribute is distinguishable from one that
// was accepted and lowered.
struct CountingDiagConsumer : clang::DiagnosticConsumer {
  int errors = 0;
  void HandleDiagnostic(clang::DiagnosticsEngine::Level level,
                        const clang::Diagnostic&) override {
    if (level >= clang::DiagnosticsEngine::Error) ++errors;
  }
};

TEST(Rewrite, OutOfRangePositionalIntIsRejected) {
  // -1 is the "omitted" sentinel for the positional factor/dim pair. Any
  // other negative used to be zero-extended into the uint32 and emitted
  // verbatim (`factor = -7`), because factor was parsed as a plain uint32
  // while dim went through the sentinel-aware parser.
  const std::string code = std::string(kTapaStubDecls) + R"cpp(
    void Task(tapa::ostream<float>& out) {
      [[tapa::partition("cyclic", -7)]] float a[32];
      out.write(a[0]);
    }
    void Top(tapa::ostream<float>& out) { tapa::task().invoke(Task, out); }
  )cpp";
  CountingDiagConsumer diag;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), &diag);
  ASSERT_NE(ast, nullptr);
  EXPECT_GT(diag.errors, 0) << "a factor below -1 must be diagnosed";
}

TEST(Rewrite, LowerTaskGetsFifoPragmas) {
  auto e = Build();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code = EmitTaskCode(e.program, e.program.tasks.at("Add"),
                                        backend, e.ast->getASTContext());

  // istream ports get ap_fifo interface + peek pragmas and empty() stubs.
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_fifo port = a._"));
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_fifo port = a._peek"));
  EXPECT_TRUE(Contains(code, "void(a._.empty());"));
  // ostream port gets a full() stub.
  EXPECT_TRUE(Contains(code, "void(c._.full());"));
  // The pipeline-free loop body stays; the task keeps its computation.
  EXPECT_TRUE(Contains(code, "a.read()"));
}

TEST(Rewrite, OtherTasksStrippedToSignatures) {
  auto e = Build();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code = EmitTaskCode(e.program, e.program.tasks.at("Add"),
                                        backend, e.ast->getASTContext());
  // Mmap2Stream is not the current task: its body becomes ";", so its loop is
  // gone.
  EXPECT_FALSE(Contains(code, "out.write(mem[i])"));
  // VecAdd's task() connection is gone too (stripped).
  EXPECT_FALSE(Contains(code, ".invoke(Mmap2Stream"));
}

TEST(Rewrite, UpperTaskBecomesShellWithOffsets) {
  auto e = Build();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code = EmitTaskCode(e.program, e.program.tasks.at("VecAdd"),
                                        backend, e.ast->getASTContext());

  // mmap parameters are lowered to uint64 offsets in the signature.
  EXPECT_TRUE(Contains(code, "uint64_t a_offset"));
  // The body is replaced by an interface shell (no task() / invoke left).
  EXPECT_FALSE(Contains(code, ".invoke("));
  // Middle-level scalar/offset ports get ap_none register pragmas.
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_none port = a_offset"));
  EXPECT_TRUE(Contains(code, "#pragma HLS interface ap_none port = n"));
}

TEST(Rewrite, UnreachableTaskIsStrippedLikeOtherTasks) {
  auto e = Build();
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string code = EmitTaskCode(e.program, e.program.tasks.at("Add"),
                                        backend, e.ast->getASTContext());

  // UnusedTask is unreachable from the top, but its body still invokes
  // sub-tasks with mmap args; it must be stripped like any non-current task
  // or the uint64_t sub-task signatures no longer type-check.
  EXPECT_TRUE(Contains(code, "UnusedTask(uint64_t a_offset"));
  EXPECT_FALSE(Contains(code, ".invoke("));
}

// An attribute Clang itself drops: [[tapa::partition]] applies to variables,
// so on a function it never reaches the AST -- and no removal pass can see
// what is not there. Its text stays in the buffer the rewriter copies out,
// and the vendor, for which an unknown attribute is not an error, ignores a
// directive the user wrote. The emitted code is checked for exactly this.
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
    tapa::stream<float, 2> q;
    tapa::task().invoke(LeakTask, mem, q, n);
  }
)cpp";

struct CollectingDiagConsumer : clang::DiagnosticConsumer {
  std::vector<std::string> errors;

  void HandleDiagnostic(clang::DiagnosticsEngine::Level level,
                        const clang::Diagnostic& info) override {
    if (level < clang::DiagnosticsEngine::Error) return;
    llvm::SmallVector<char, 128> msg;
    info.FormatDiagnostic(msg);
    errors.emplace_back(msg.data(), msg.size());
  }
};

TEST(Rewrite, AttrThatCannotLowerIsAnError) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kLeakedAttr;
  // The consumer outlives the ASTUnit's diagnostics engine, which keeps
  // pointing at it while EmitTaskCode reports.
  auto diag = std::make_unique<CollectingDiagConsumer>();
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), diag.get());
  ASSERT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "LeakTop", SynthTarget::kXilinxHls);
  const XilinxBackend backend(/*is_vitis=*/false);

  diag->errors.clear();
  const std::string emitted = EmitTaskCode(
      program, program.tasks.at("LeakTask"), backend, ast->getASTContext());

  EXPECT_TRUE(Contains(emitted, "[[tapa::partition"));
  ASSERT_EQ(diag->errors.size(), 1u);
  EXPECT_TRUE(Contains(diag->errors.front(), "[[tapa::partition]]"));
  EXPECT_TRUE(Contains(diag->errors.front(), "LeakTask"));
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
      tapa::stream<float, 2> q;
      tapa::task().invoke(QuotedTask, mem, q, n);
    }
  )cpp";
  const std::string code = std::string(kTapaStubDecls) + "\n" + kCommentedAttr;
  auto diag = std::make_unique<CollectingDiagConsumer>();
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), diag.get());
  ASSERT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "QuotedTop", SynthTarget::kXilinxHls);
  const XilinxBackend backend(/*is_vitis=*/false);

  diag->errors.clear();
  const std::string emitted = EmitTaskCode(
      program, program.tasks.at("QuotedTask"), backend, ast->getASTContext());

  EXPECT_TRUE(Contains(emitted, "[[tapa::pipeline")) << emitted;
  EXPECT_TRUE(diag->errors.empty())
      << (diag->errors.empty() ? "" : diag->errors.front());
}

// A forward declaration shares the definition's body through the
// redeclaration chain, so rewriting it too used to insert every stream
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
      tapa::stream<float, 2> q;
      tapa::task().invoke(FwdTask, mem, q, n);
    }
  )cpp";
  const std::string code = std::string(kTapaStubDecls) + "\n" + kFwdHelper;
  PrintingDiagConsumer diag;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), &diag);
  ASSERT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "FwdTop", SynthTarget::kXilinxHls);
  const XilinxBackend backend(/*is_vitis=*/false);
  const std::string emitted = EmitTaskCode(program, program.tasks.at("FwdTask"),
                                           backend, ast->getASTContext());

  const std::string pragma = "HLS stream variable = s";
  size_t count = 0;
  for (size_t pos = emitted.find(pragma); pos != std::string::npos;
       pos = emitted.find(pragma, pos + pragma.size())) {
    ++count;
  }
  EXPECT_EQ(count, 1u) << emitted;
}

// `[[tapa::target]]` also survives into the emitted source, and is the one
// spelling that should: it picks the backend, which discovery has already
// acted on, so there is nothing left to lower and nothing lost.
constexpr char kTargetAttr[] = R"cpp(
  [[tapa::target("ignore")]] void TargetTask(tapa::mmap<const float> mem,
                                             tapa::ostream<float>& out,
                                             unsigned long long n) {
    for (unsigned long long i = 0; i < n; ++i) out.write(mem[i]);
  }
  void TargetTop(tapa::mmap<const float> mem, tapa::ostream<float>& out,
                 unsigned long long n) {
    tapa::stream<float, 2> q;
    tapa::task().invoke(TargetTask, mem, q, n);
  }
)cpp";

TEST(Rewrite, TargetAttrIsNotReportedAsALeak) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kTargetAttr;
  auto diag = std::make_unique<CollectingDiagConsumer>();
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), diag.get());
  ASSERT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "TargetTop", SynthTarget::kXilinxHls);
  const XilinxBackend backend(/*is_vitis=*/false);

  diag->errors.clear();
  const std::string emitted = EmitTaskCode(
      program, program.tasks.at("TargetTask"), backend, ast->getASTContext());

  EXPECT_TRUE(Contains(emitted, "[[tapa::target"));
  EXPECT_TRUE(diag->errors.empty());
}

}  // namespace
}  // namespace tapa::cc
