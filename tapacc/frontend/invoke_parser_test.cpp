#include "invoke_parser.h"

#include <map>
#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "discover.h"
#include "program.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

class FuncCollector : public clang::RecursiveASTVisitor<FuncCollector> {
 public:
  explicit FuncCollector(const clang::ASTContext& ctx) : ctx_(ctx) {}
  std::vector<const clang::FunctionDecl*> funcs;
  bool VisitFunctionDecl(clang::FunctionDecl* f) {
    if (f->isGlobal() &&
        ctx_.getSourceManager().isWrittenInMainFile(f->getLocation()) &&
        f->hasBody()) {
      funcs.push_back(f);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
};

constexpr char kProgram[] = R"cpp(
  void Producer(tapa::ostream<float>& out) {}
  void Consumer(tapa::istream<float>& in) {}
  void Adder(tapa::istream<float>& a, tapa::istream<float>& b,
             tapa::ostream<float>& c, unsigned long long n) {}
  void Top() {
    tapa::stream<float, 8> q1;
    tapa::stream<float, 8> q2;
    tapa::stream<float, 16> qc;
    tapa::task()
        .invoke(Producer, q1)
        .invoke(Producer, q2)
        .invoke(Adder, q1, q2, qc, 100)
        .invoke(Consumer, qc);
  }
)cpp";

struct Parsed {
  std::unique_ptr<clang::ASTUnit> ast;
  std::map<std::string, TaskModel> tasks;
};

Parsed ParseTop() {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kProgram;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  FuncCollector collector(ast->getASTContext());
  collector.TraverseDecl(ast->getASTContext().getTranslationUnitDecl());
  auto tasks = DiscoverTasks(ast->getASTContext(), "Top",
                             SynthTarget::kXilinxHls, collector.funcs);
  for (auto& [name, model] : tasks) {
    if (model.level == TaskLevel::kUpper) {
      ParseUpperTask(ast->getASTContext(), model);
    }
  }
  return Parsed{std::move(ast), std::move(tasks)};
}

TEST(InvokeParser, StreamsWithDepth) {
  auto p = ParseTop();
  const TaskModel& top = p.tasks.at("Top");
  ASSERT_EQ(top.streams.size(), 3u);
  EXPECT_EQ(top.streams.at("q1").depth, 8u);
  EXPECT_EQ(top.streams.at("q2").depth, 8u);
  EXPECT_EQ(top.streams.at("qc").depth, 16u);
}

TEST(InvokeParser, ProducerConsumerEndpoints) {
  auto p = ParseTop();
  const TaskModel& top = p.tasks.at("Top");

  const StreamDecl& q1 = top.streams.at("q1");
  ASSERT_TRUE(q1.produced_by.has_value());
  ASSERT_TRUE(q1.consumed_by.has_value());
  EXPECT_EQ(q1.produced_by->task, "Producer");
  EXPECT_EQ(q1.produced_by->index, 0u);
  EXPECT_EQ(q1.consumed_by->task, "Adder");
  EXPECT_EQ(q1.consumed_by->index, 0u);

  const StreamDecl& q2 = top.streams.at("q2");
  EXPECT_EQ(q2.produced_by->task, "Producer");
  EXPECT_EQ(q2.produced_by->index, 1u);  // second Producer instance

  const StreamDecl& qc = top.streams.at("qc");
  EXPECT_EQ(qc.produced_by->task, "Adder");
  EXPECT_EQ(qc.consumed_by->task, "Consumer");
}

TEST(InvokeParser, InstancesAndArgs) {
  auto p = ParseTop();
  const TaskModel& top = p.tasks.at("Top");

  ASSERT_EQ(top.instances.at("Producer").size(), 2u);
  EXPECT_EQ(top.instances.at("Producer")[0].args.at("out").arg, "q1");
  EXPECT_EQ(top.instances.at("Producer")[1].args.at("out").arg, "q2");

  ASSERT_EQ(top.instances.at("Adder").size(), 1u);
  const Instance& adder = top.instances.at("Adder")[0];
  EXPECT_EQ(adder.step, 0);  // join
  EXPECT_EQ(adder.args.at("a").arg, "q1");
  EXPECT_EQ(adder.args.at("b").arg, "q2");
  EXPECT_EQ(adder.args.at("c").arg, "qc");
  EXPECT_EQ(adder.args.at("n").arg, "64'd100");
  EXPECT_EQ(adder.args.at("n").cat, TapaKind::kNotTapa);

  ASSERT_EQ(top.instances.at("Consumer").size(), 1u);
  EXPECT_EQ(top.instances.at("Consumer")[0].args.at("in").arg, "qc");
}

}  // namespace
}  // namespace tapa::cc
