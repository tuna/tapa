// The typed program model one TU yields: task set, levels, ports, streams,
// and instances, asserted on the builder after the full pipeline ran.

#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "nlohmann/json.hpp"

#include "classify.h"
#include "codegen/tree_pipeline.h"
#include "program.h"

namespace tapa::cc {
namespace {

using test_support::TreePipelineRun;
using test_support::VirtualTu;

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

TreePipelineRun Build() {
  auto run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/vadd.cpp", kVadd}}, "VecAdd",
      testing::TempDir() + "/build_program_vadd", {"-std=c++17"});
  EXPECT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  return run;
}

TEST(BuildProgram, TopAndTaskSet) {
  const TreePipelineRun b = Build();
  EXPECT_EQ(b.builder.top(), "VecAdd");
  EXPECT_EQ(nlohmann::json::parse(b.json)["tasks"].size(), 4u);
  EXPECT_NE(b.builder.FindTask("VecAdd"), nullptr);
  EXPECT_EQ(b.builder.FindTask("VecAdd")->level, TaskLevel::kUpper);
  EXPECT_EQ(b.builder.FindTask("Add")->level, TaskLevel::kLower);
  EXPECT_EQ(b.builder.FindTask("Mmap2Stream")->level, TaskLevel::kLower);
  EXPECT_EQ(b.builder.FindTask("Nope"), nullptr);
}

TEST(BuildProgram, TopPorts) {
  const TreePipelineRun b = Build();
  const std::vector<Port>& ports = b.builder.FindTask("VecAdd")->ports;
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
  const TreePipelineRun b = Build();
  const TaskModel& top = *b.builder.FindTask("VecAdd");

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
  const TreePipelineRun b = Build();
  const std::vector<Port>& add_ports = b.builder.FindTask("Add")->ports;
  ASSERT_EQ(add_ports.size(), 4u);
  EXPECT_STREQ(TapaKindCat(add_ports[0].kind), "istream");
  EXPECT_STREQ(TapaKindCat(add_ports[2].kind), "ostream");
  EXPECT_STREQ(TapaKindCat(add_ports[3].kind), "scalar");
}

}  // namespace
}  // namespace tapa::cc
