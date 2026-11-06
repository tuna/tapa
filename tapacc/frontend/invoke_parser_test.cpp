#include "invoke_parser.h"

#include <map>
#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/Tooling/Tooling.h"

#include "codegen/tree_pipeline.h"
#include "codegen/tree_writer.h"
#include "ports.h"
#include "program.h"
#include "program_builder.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

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
  test_support::TreePipelineRun run;

  const TaskModel& Task(llvm::StringRef name) const {
    const TaskModel* model = run.builder.FindTask(name.str());
    EXPECT_NE(model, nullptr);
    return *model;
  }
};

// Full pipeline over one TU: index, merge, rewrite (which fills ports,
// instances, and streams, with is_top true exactly for the requested top).
Parsed ParseCode(llvm::StringRef code, llvm::StringRef top) {
  auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/invoke.cpp", code.str()}}, top.str(),
      testing::TempDir() + "/invoke_parser", {"-std=c++17"});
  EXPECT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  return Parsed{std::move(run)};
}

Parsed ParseTop() { return ParseCode(kProgram, "Top"); }

TEST(InvokeParser, StreamsWithDepth) {
  auto p = ParseTop();
  const TaskModel& top = p.Task("Top");
  ASSERT_EQ(top.streams.size(), 3u);
  EXPECT_EQ(top.streams.at("q1").depth, 8u);
  EXPECT_EQ(top.streams.at("q2").depth, 8u);
  EXPECT_EQ(top.streams.at("qc").depth, 16u);
}

constexpr char kTopStreamProgram[] = R"cpp(
  void Adder(tapa::istream<float>& a, tapa::istream<float>& b,
             tapa::ostream<float>& c) {}
  void Top(tapa::istream<float>& a, tapa::istream<float>& b,
           tapa::ostream<float>& c) {
    tapa::task().invoke(Adder, a, b, c);
  }
)cpp";

TEST(InvokeParser, TopLevelStreamPortsBecomeExternalFifos) {
  auto p = ParseCode(kTopStreamProgram, "Top");
  const TaskModel& top = p.Task("Top");
  ASSERT_EQ(top.streams.size(), 3u);

  const StreamDecl& a = top.streams.at("a");
  EXPECT_FALSE(a.depth.has_value());  // external FIFO: no depth
  EXPECT_FALSE(a.produced_by.has_value());
  ASSERT_TRUE(a.consumed_by.has_value());
  EXPECT_EQ(a.consumed_by->task, "Adder");
  EXPECT_EQ(a.consumed_by->index, 0u);

  const StreamDecl& b = top.streams.at("b");
  ASSERT_TRUE(b.consumed_by.has_value());
  EXPECT_EQ(b.consumed_by->task, "Adder");

  const StreamDecl& c = top.streams.at("c");
  EXPECT_FALSE(c.depth.has_value());
  EXPECT_FALSE(c.consumed_by.has_value());
  ASSERT_TRUE(c.produced_by.has_value());
  EXPECT_EQ(c.produced_by->task, "Adder");
  EXPECT_EQ(c.produced_by->index, 0u);
}

TEST(InvokeParser, NonTopStreamPortsDoNotBecomeFifos) {
  // The same program with the top flag OFF: passthrough stream ports stay
  // out of `streams` (middle tasks bind them by port name instead). Drives
  // ParseUpperTask directly because the builder always marks the requested
  // top as the top — the is_top distinction belongs to this unit.
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      std::string(kTapaStubDecls) + "\n" + kTopStreamProgram,
      std::vector<std::string>{"-std=c++17"}, "/proj/nontop.cpp");
  ASSERT_NE(ast, nullptr);
  const std::vector<IncludeDirective> log;  // no includes
  ProgramBuilder builder(
      "Top", SynthTarget::kXilinxHls,
      TreeConfig{testing::TempDir() + "/nontop", {"/proj/nontop.cpp"}});
  builder.IndexTu(ast->getASTContext(), log);
  ASSERT_TRUE(builder.MergeAndDiscover());
  TaskModel model = builder.TuView(ast->getASTContext(), log).tasks.at("Top");
  model.ports = BuildPorts(ast->getASTContext(), model.def);
  ParseUpperTask(ast->getASTContext(), model, /*is_top=*/false,
                 [](const clang::FunctionDecl* callee, unsigned index) {
                   return callee->getParamDecl(index)->getNameAsString();
                 });
  EXPECT_TRUE(model.streams.empty());
}

TEST(InvokeParser, ProducerConsumerEndpoints) {
  auto p = ParseTop();
  const TaskModel& top = p.Task("Top");

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

TEST(InvokeParser, ReplicatedExplicitNameIsUniquePerLane) {
  constexpr char kCode[] = R"cpp(
    void Worker(int value) {}
    void Top() { tapa::task().invoke<-1, 3>(Worker, "worker", 42); }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& instances = p.Task("Top").instances.at("Worker");
  ASSERT_EQ(instances.size(), 3u);
  EXPECT_EQ(instances[0].name, "worker_0");
  EXPECT_EQ(instances[1].name, "worker_1");
  EXPECT_EQ(instances[2].name, "worker_2");
}

// A host program running an upper task's body finds each invoke by where
// it sits in the chain, so positions follow the source across task names
// and count every lane of a vectorized invoke.
TEST(InvokeParser, PositionsFollowTheInvokeChain) {
  constexpr char kCode[] = R"cpp(
    void Worker(int value) {}
    void Other() {}
    void Top() { tapa::task().invoke(Other).invoke<0, 2>(Worker, 1).invoke(Other); }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& others = p.Task("Top").instances.at("Other");
  const auto& workers = p.Task("Top").instances.at("Worker");
  ASSERT_EQ(others.size(), 2u);
  ASSERT_EQ(workers.size(), 2u);
  EXPECT_EQ(others[0].position, 0u);
  EXPECT_EQ(workers[0].position, 1u);
  EXPECT_EQ(workers[1].position, 2u);
  EXPECT_EQ(others[1].position, 3u);
}

TEST(InvokeParser, NameOnlyInvokeNamesOneInstance) {
  // The name-only overload specializes as [Func, name_size] with an empty
  // Args pack; reading template arguments positionally would misread the
  // name's length as a vector length and spawn one instance per character.
  constexpr char kCode[] = R"cpp(
    void Worker() {}
    void Top() { tapa::task().invoke(Worker, "worker"); }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& instances = p.Task("Top").instances.at("Worker");
  ASSERT_EQ(instances.size(), 1u);
  EXPECT_EQ(instances[0].name, "worker");
}

TEST(InvokeParser, ModeWithNameKeepsBoth) {
  constexpr char kCode[] = R"cpp(
    void Worker(int value) {}
    void Top() { tapa::task().invoke<-1>(Worker, "worker", 42); }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& instances = p.Task("Top").instances.at("Worker");
  ASSERT_EQ(instances.size(), 1u);
  EXPECT_EQ(instances[0].name, "worker");
}

TEST(InvokeParser, ConstantUsesChildPortWidth) {
  constexpr char kCode[] = R"cpp(
    void Worker(short value) {}
    void Top() { tapa::task().invoke(Worker, -1); }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& arg = p.Task("Top").instances.at("Worker")[0].args.at("value");
  EXPECT_EQ(arg.width, 16u);
  EXPECT_EQ(arg.value, std::optional<uint64_t>(0xffff));
}

TEST(InvokeParser, ConstantTakesTheLanguageConversionToThePort) {
  // The recorded bits exercise conversions to the child port's type.
  // `invoke` binds `Args&&...`, so nothing in the AST narrows these -- the
  // frontend asks clang to.
  constexpr char kCode[] = R"cpp(
    void Narrow(unsigned char value) {}
    void Signed(signed char value) {}
    void Flag(bool value) {}
    void Wide(long long value) {}
    void UnsignedWide(unsigned long long value) {}
    void Float(float value) {}
    void Double(double value) {}
    constexpr unsigned kHigh = 0xffffffffU;
    const int kNegative = -1;
    void Top() {
      constexpr int kRounded = 16777217;
      tapa::task()
          .invoke(Narrow, 300)
          .invoke(Signed, 200)
          .invoke(Flag, 2)
          .invoke(Wide, -1)
          .invoke(UnsignedWide, 0xffffffffffffffffULL)
          .invoke(Float, 16777217)
          .invoke(Float, -1)
          .invoke(Double, -1)
          .invoke(Double, 0xffffffffU)
          .invoke(Float, kRounded)
          .invoke(Double, kHigh)
          .invoke(Float, kNegative);
    }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const auto& insts = p.Task("Top").instances;
  auto value_of = [&](const char* task, unsigned index = 0) {
    return insts.at(task)[index].args.at("value").value;
  };

  // 300 -> 8 bits keeps the low byte; 200 -> signed char is 0xc8 either way.
  EXPECT_EQ(value_of("Narrow"), std::optional<uint64_t>(300 & 0xff));
  EXPECT_EQ(value_of("Signed"), std::optional<uint64_t>(0xc8));
  // A boolean conversion, not a truncation: 2 is `true`, not its low bit.
  EXPECT_EQ(value_of("Flag"), std::optional<uint64_t>(1));
  // No truncation at 64 bits: the full two's-complement pattern survives.
  EXPECT_EQ(value_of("Wide"), std::optional<uint64_t>(0xffffffffffffffffULL));
  // The upper half of an unsigned 64-bit port must survive extraction too.
  EXPECT_EQ(value_of("UnsignedWide"),
            std::optional<uint64_t>(0xffffffffffffffffULL));
  // Float rounds 2^24 + 1 to 2^24 rather than keeping the integer's bits.
  EXPECT_EQ(value_of("Float"), std::optional<uint64_t>(0x4b800000));
  // Signed negative sources must stay negative in either floating format.
  EXPECT_EQ(value_of("Float", 1), std::optional<uint64_t>(0xbf800000));
  EXPECT_EQ(value_of("Double"), std::optional<uint64_t>(0xbff0000000000000ULL));
  // The unsigned source's high bit must stay positive, unlike a signed -1.
  EXPECT_EQ(value_of("Double", 1),
            std::optional<uint64_t>(0x41efffffffe00000ULL));
  // Named compile-time constants use the same conversion as literal sources.
  EXPECT_EQ(value_of("Float", 2), std::optional<uint64_t>(0x4b800000));
  EXPECT_EQ(value_of("Double", 2),
            std::optional<uint64_t>(0x41efffffffe00000ULL));
  EXPECT_EQ(value_of("Float", 3), std::optional<uint64_t>(0xbf800000));
}

TEST(InvokeParser, InstancesAndArgs) {
  auto p = ParseTop();
  const TaskModel& top = p.Task("Top");

  ASSERT_EQ(top.instances.at("Producer").size(), 2u);
  EXPECT_EQ(top.instances.at("Producer")[0].args.at("out").arg, "q1");
  EXPECT_EQ(top.instances.at("Producer")[1].args.at("out").arg, "q2");

  ASSERT_EQ(top.instances.at("Adder").size(), 1u);
  const Instance& adder = top.instances.at("Adder")[0];
  EXPECT_EQ(adder.step, 0);  // join
  EXPECT_EQ(adder.args.at("a").arg, "q1");
  EXPECT_EQ(adder.args.at("b").arg, "q2");
  EXPECT_EQ(adder.args.at("c").arg, "qc");
  // An integer constant leaves the frontend as a width and a value, not as
  // Verilog text.
  EXPECT_TRUE(adder.args.at("n").arg.empty());
  EXPECT_EQ(adder.args.at("n").value, std::optional<uint64_t>(100));
  EXPECT_EQ(adder.args.at("n").width, 64u);
  EXPECT_EQ(adder.args.at("n").cat, TapaKind::kNotTapa);

  ASSERT_EQ(top.instances.at("Consumer").size(), 1u);
  EXPECT_EQ(top.instances.at("Consumer")[0].args.at("in").arg, "qc");
}

// Parse `code` expecting upper-task diagnostics; returns the error count
// the pipeline collected (the harness fails a run on any error diagnostic,
// so the count is what distinguishes rejected from accepted).
unsigned CountUpperTaskErrors(llvm::StringRef code, llvm::StringRef top) {
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/invoke_err.cpp", code.str()}}, top.str(),
      testing::TempDir() + "/invoke_parser_err", {"-std=c++17"});
  return static_cast<unsigned>(run.diags.size());
}

TEST(InvokeParser, RuntimeScalarBindingsRequireBitPreservingTypes) {
  constexpr char kForward[] = R"cpp(
    using Scalar = float;
    enum class Mode : unsigned { First };
    void Float(const Scalar value) {}
    void Integer(unsigned value) {}
    void Enum(Mode value) {}
    void Top(const float value, int integer, Mode mode) {
      tapa::task().invoke(Float, value).invoke(Integer, integer).invoke(Enum, mode);
    }
  )cpp";
  auto p = ParseCode(kForward, "Top");
  const auto& insts = p.Task("Top").instances;
  EXPECT_EQ(insts.at("Float")[0].args.at("value").arg, "value");
  EXPECT_EQ(insts.at("Integer")[0].args.at("value").arg, "integer");
  EXPECT_EQ(insts.at("Enum")[0].args.at("value").arg, "mode");

  constexpr char kConvert[] = R"cpp(
    enum class Mode : unsigned { First };
    void Float(float value) {}
    void Double(double value) {}
    void Integer(int value) {}
    void Flag(bool value) {}
    void Wide(long long value) {}
    void Narrow(unsigned short value) {}
    void UnsignedWide(unsigned long long value) {}
    void Enum(Mode value) {}
    void Top(int integer, const int readonly, float fp, double dp,
             unsigned unsigned_value) {
      const int runtime = integer;
      tapa::task()
          .invoke(Float, integer)
          .invoke(Float, readonly)
          .invoke(Float, runtime)
          .invoke(Integer, fp)
          .invoke(Double, fp)
          .invoke(Float, dp)
          .invoke(Flag, integer)
          .invoke(Wide, integer)
          .invoke(Narrow, unsigned_value)
          .invoke(UnsignedWide, unsigned_value)
          .invoke(Enum, unsigned_value);
    }
  )cpp";
  auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/scalar_conversion.cpp", kConvert}}, "Top",
      testing::TempDir() + "/scalar_conversion", {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_EQ(run.diags.size(), 11u);
  for (const auto& diag : run.diags) {
    EXPECT_NE(diag.find("unsupported runtime scalar conversion"),
              std::string::npos)
        << diag;
  }
}

TEST(InvokeParser, FloatingConstantsCannotBecomeRuntimeBindings) {
  constexpr char kCode[] = R"cpp(
    using Scalar = float;
    constexpr Scalar kGlobal = 1.5f;
    const double kDouble = 2.5;
    void Float(float value) {}
    void Double(double value) {}
    void Top(const Scalar runtime, const double wide) {
      constexpr Scalar kLocal = 3.5f;
      tapa::task()
          .invoke(Float, kGlobal)
          .invoke(Double, kDouble)
          .invoke(Float, kLocal)
          .invoke(Float, runtime)
          .invoke(Double, wide);
    }
  )cpp";
  const auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/floating_constant.cpp", kCode}}, "Top",
      testing::TempDir() + "/floating_constant", {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_EQ(run.diags.size(), 3u);
  for (const auto& diag : run.diags) {
    EXPECT_NE(diag.find("floating-point constants are unsupported"),
              std::string::npos)
        << diag;
  }
  // Read-only parent formals still have real RTL ports and remain forwardable.
  const TaskModel* top = run.builder.FindTask("Top");
  ASSERT_NE(top, nullptr);
  EXPECT_EQ(top->instances.at("Float").at(2).args.at("value").arg, "runtime");
  EXPECT_EQ(top->instances.at("Double").at(1).args.at("value").arg, "wide");
}

TEST(InvokeParser, RuntimeScalarsMustReferToParentParameters) {
  constexpr char kCode[] = R"cpp(
    namespace cfg {
    int value = 7;
    }
    void Worker(int value) {}
    void Top(const int value) {
      const int runtime = value;
      tapa::task()
          .invoke(Worker, cfg::value)
          .invoke(Worker, runtime)
          .invoke(Worker, value);
    }
  )cpp";
  const auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/scalar_origin.cpp", kCode}}, "Top",
      testing::TempDir() + "/scalar_origin", {"-std=c++17"});
  EXPECT_FALSE(run.ok);
  ASSERT_EQ(run.diags.size(), 2u);
  for (const auto& diag : run.diags) {
    EXPECT_NE(diag.find("must name a parent task parameter"), std::string::npos)
        << diag;
  }
  const TaskModel* top = run.builder.FindTask("Top");
  ASSERT_NE(top, nullptr);
  EXPECT_EQ(top->instances.at("Worker").at(2).args.at("value").arg, "value");
}

TEST(InvokeParser, NestedScopeStreamDeclIsAnErrorNotSilence) {
  // A stream declared inside a scope block is invisible to
  // CollectStreamDecls' direct-children walk; without the diagnostic it
  // would silently drop from the task graph.
  constexpr char kCode[] = R"cpp(
    void Producer(tapa::ostream<float>& out) {}
    void Consumer(tapa::istream<float>& in) {}
    void Top() {
      tapa::stream<float, 8> ok;
      if (true) {
        tapa::stream<float, 8> hidden;
      }
      tapa::task().invoke(Producer, ok).invoke(Consumer, ok);
    }
  )cpp";
  EXPECT_GE(CountUpperTaskErrors(kCode, "Top"), 1u);
}

TEST(InvokeParser, MultiDeclaratorStreamDeclCollectsEveryDeclarator) {
  // `tapa::stream<float> a("a"), b("b");` is user-exercised (see
  // tests/functional/multi-file); every declarator must be collected — a
  // first-declarator-only walk would silently drop the rest of the statement.
  constexpr char kCode[] = R"cpp(
    void Producer(tapa::ostream<float>& out) {}
    void Consumer(tapa::istream<float>& in) {}
    void Top() {
      tapa::stream<float, 8> q1, q2;
      tapa::task()
          .invoke(Producer, q1)
          .invoke(Consumer, q1)
          .invoke(Producer, q2)
          .invoke(Consumer, q2);
    }
  )cpp";
  auto p = ParseCode(kCode, "Top");
  const TaskModel& top = p.Task("Top");
  ASSERT_EQ(top.streams.size(), 2u);
  EXPECT_EQ(top.streams.at("q1").depth, 8u);
  EXPECT_EQ(top.streams.at("q2").depth, 8u);
}

}  // namespace
}  // namespace tapa::cc
