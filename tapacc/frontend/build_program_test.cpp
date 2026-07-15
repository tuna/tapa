#include "build_program.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "classify.h"
#include "program.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

constexpr char kVadd[] = R"cpp(
  void Mmap2Stream(tapa::mmap<const float> mem, unsigned long long n,
                   tapa::ostream<float>& out) {}
  void Stream2Mmap(tapa::istream<float>& in, tapa::mmap<float> mem,
                   unsigned long long n) {}
  void Add(tapa::istream<float>& a, tapa::istream<float>& b,
           tapa::ostream<float>& c, unsigned long long n) {}
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

struct Built {
  std::unique_ptr<clang::ASTUnit> ast;
  Program program;
};

Built Build() {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kVadd;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  Program program =
      BuildProgram(ast->getASTContext(), "VecAdd", SynthTarget::kXilinxHls);
  return Built{std::move(ast), std::move(program)};
}

TEST(BuildProgram, TopAndTaskSet) {
  auto b = Build();
  EXPECT_EQ(b.program.top, "VecAdd");
  EXPECT_EQ(b.program.tasks.size(), 4u);
  EXPECT_EQ(b.program.tasks.at("VecAdd").level, TaskLevel::kUpper);
  EXPECT_EQ(b.program.tasks.at("Add").level, TaskLevel::kLower);
  EXPECT_EQ(b.program.tasks.at("Mmap2Stream").level, TaskLevel::kLower);
}

TEST(BuildProgram, TopPorts) {
  auto b = Build();
  const std::vector<Port>& ports = b.program.tasks.at("VecAdd").ports;
  ASSERT_EQ(ports.size(), 4u);
  EXPECT_EQ(ports[0].name, "a");
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "mmap");
  EXPECT_EQ(ports[0].ctype, "const float*");
  EXPECT_EQ(ports[2].name, "c");
  EXPECT_EQ(ports[2].ctype, "float*");
  EXPECT_EQ(ports[3].name, "n");
  EXPECT_STREQ(TapaKindCat(ports[3].kind), "scalar");
}

TEST(BuildProgram, UpperStreamsAndInstances) {
  auto b = Build();
  const TaskModel& top = b.program.tasks.at("VecAdd");

  ASSERT_EQ(top.streams.size(), 3u);
  EXPECT_EQ(top.streams.at("a_q").depth, 8u);

  EXPECT_EQ(top.instances.at("Mmap2Stream").size(), 2u);
  EXPECT_EQ(top.instances.at("Add").size(), 1u);
  EXPECT_EQ(top.instances.at("Stream2Mmap").size(), 1u);

  const Instance& add = top.instances.at("Add")[0];
  EXPECT_EQ(add.args.at("a").arg, "a_q");
  EXPECT_EQ(add.args.at("b").arg, "b_q");
  EXPECT_EQ(add.args.at("c").arg, "c_q");
  EXPECT_EQ(add.args.at("n").arg, "n");

  // a_q: produced by the first Mmap2Stream, consumed by Add.
  const StreamDecl& a_q = top.streams.at("a_q");
  ASSERT_TRUE(a_q.produced_by.has_value());
  ASSERT_TRUE(a_q.consumed_by.has_value());
  EXPECT_EQ(a_q.produced_by->task, "Mmap2Stream");
  EXPECT_EQ(a_q.consumed_by->task, "Add");
}

TEST(BuildProgram, LeafPortsPopulated) {
  auto b = Build();
  const std::vector<Port>& add_ports = b.program.tasks.at("Add").ports;
  ASSERT_EQ(add_ports.size(), 4u);
  EXPECT_STREQ(TapaKindCat(add_ports[0].kind), "istream");
  EXPECT_STREQ(TapaKindCat(add_ports[2].kind), "ostream");
  EXPECT_STREQ(TapaKindCat(add_ports[3].kind), "scalar");
}

}  // namespace
}  // namespace tapa::cc
