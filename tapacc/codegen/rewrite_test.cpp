#include "rewrite.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "../frontend/build_program.h"
#include "../frontend/program.h"
#include "../frontend/tapa_stub_decls.h"
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

}  // namespace
}  // namespace tapa::cc
