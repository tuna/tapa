#include "invoke_parser.h"

#include <map>
#include <memory>
#include <set>
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
  // The local copy `runtime` is reported first: hardware would drop it.
  ASSERT_EQ(run.diags.size(), 12u);
  EXPECT_NE(run.diags.front().find("computes outside its invokes"),
            std::string::npos)
      << run.diags.front();
  for (size_t i = 1; i < run.diags.size(); ++i) {
    EXPECT_NE(run.diags[i].find("unsupported runtime scalar conversion"),
              std::string::npos)
        << run.diags[i];
  }
}

TEST(InvokeParser, ArbitraryPrecisionIntsForwardAtTheirDeclaredWidth) {
  // tapa::u<W> and tapa::i<W> are W-bit integers, so raw forwarding needs
  // only an equal width, as between builtin integers; a real design
  // (tests/regression/serpens-32ch) hands `int` to `tapa::u<32>` and
  // `short` to `tapa::u<16>`.
  constexpr char kCode[] = R"cpp(
    namespace tapa {
    template <int W>
    struct u {
      u(long long);
      unsigned long long v;
    };
    template <int W>
    struct i {
      i(long long);
      long long v;
    };
    }  // namespace tapa
    void Narrow(const tapa::u<16> value) {}
    void Word(tapa::u<32> value) {}
    void Signed(tapa::i<32> value) {}
    void Back(int value) {}
    void Top(short half, int word, tapa::u<32> raw, tapa::i<31> odd) {
      tapa::task()
          .invoke(Narrow, half)
          .invoke(Word, word)
          .invoke(Signed, raw)
          .invoke(Back, raw)
          .invoke(Word, half)
          .invoke(Signed, odd);
    }
  )cpp";
  const auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/arbitrary_int.cpp", kCode}}, "Top",
      testing::TempDir() + "/arbitrary_int", {"-std=c++17"});
  // Only the two width changes are rejected.
  ASSERT_EQ(run.diags.size(), 2u) << run.diags.front();
  for (const auto& diag : run.diags) {
    EXPECT_NE(diag.find("unsupported runtime scalar conversion"),
              std::string::npos)
        << diag;
  }
  const TaskModel* top = run.builder.FindTask("Top");
  ASSERT_NE(top, nullptr);
  EXPECT_EQ(top->instances.at("Narrow").at(0).args.at("value").arg, "half");
  EXPECT_EQ(top->instances.at("Word").at(0).args.at("value").arg, "word");
  EXPECT_EQ(top->instances.at("Signed").at(0).args.at("value").arg, "raw");
  EXPECT_EQ(top->instances.at("Back").at(0).args.at("value").arg, "raw");
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
  // The local copy `runtime` is reported first: hardware would drop it.
  ASSERT_EQ(run.diags.size(), 3u);
  EXPECT_NE(run.diags.front().find("computes outside its invokes"),
            std::string::npos)
      << run.diags.front();
  for (size_t i = 1; i < run.diags.size(); ++i) {
    EXPECT_NE(run.diags[i].find("must name a parent task parameter"),
              std::string::npos)
        << run.diags[i];
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

TEST(InvokeParser, AnUpperTaskHoldsOnlyChannelsConstantsAndInvokes) {
  // Hardware keeps only these, so anything else would silently vanish from
  // the RTL; each computing statement is its own error.
  constexpr char kCode[] = R"cpp(
#define assert(condition) ((void)(condition))
    void Producer(tapa::ostream<int>& out, int n) {}
    void Consumer(tapa::istream<int>& in, int n) {}
    int Twice(int n) { return 2 * n; }
    void Top(int n) {
      using Word = int;
      static_assert(sizeof(Word) == 4, "");
      assert(n > 0);
      constexpr int kDepth = 8;
      const int kCopies = kDepth / 2;
      tapa::stream<Word, kDepth> q;
      ;
      int m = n + kCopies;
      for (int i = 0; i < n; ++i) m += i;
      Twice(m);
      tapa::task().invoke(Producer, q, n).invoke(Consumer, q, n);
      return;
    }
  )cpp";
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/upper_body.cpp", kCode}}, "Top",
      testing::TempDir() + "/upper_body", {"-std=c++17"});
  ASSERT_EQ(run.diags.size(), 3u);
  for (const std::string& diag : run.diags) {
    EXPECT_NE(diag.find("upper task 'Top' computes outside its invokes"),
              std::string::npos)
        << diag;
  }
}

TEST(InvokeParser, EveryInvokeSitsInTheFirstTaskStatement) {
  // The graph holds only the invokes of the first tapa::task() statement
  // written directly in a body; HLS runs any other invoke as a plain call.
  // Each such invoke is its own error, and one in a leaf is one too. An
  // ignored task's body is its software model and may invoke freely.
  constexpr char kCode[] = R"cpp(
    void Leaf(tapa::mmap<int> m) {}
    [[tapa::target("ignore")]] void Model(tapa::mmap<int> m) {
      tapa::task().invoke(Leaf, m);
    }
    void Nested(tapa::mmap<int> m) {
      {
        tapa::task().invoke(Leaf, m);
      }
    }
    void Looped(tapa::mmap<int> m) {
      for (int i = 0; i < 2; ++i) tapa::task().invoke(Leaf, m);
    }
    void Named(tapa::mmap<int> m, tapa::mmap<int> n) {
      tapa::task t;
      t.invoke(Leaf, m);
      t.invoke(Leaf, n);
    }
    void Top(tapa::mmap<int> a, tapa::mmap<int> b, tapa::mmap<int> c,
             tapa::mmap<int> d, tapa::mmap<int> e, tapa::mmap<int> f) {
      tapa::task().invoke(Model, a).invoke(Nested, b).invoke(Looped, c).invoke(
          Named, d, e);
      tapa::task().invoke(Leaf, f);
    }
  )cpp";
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/stray_invoke.cpp", kCode}}, "Top",
      testing::TempDir() + "/stray_invoke", {"-std=c++17"});
  // Each message names the task whose body holds the stray invoke.
  std::multiset<std::string> strays;
  for (const std::string& diag : run.diags) {
    constexpr llvm::StringLiteral kPrefix = "task '";
    if (diag.find("outside the first tapa::task() statement") ==
            std::string::npos ||
        diag.rfind(kPrefix.str(), 0) != 0) {
      continue;
    }
    strays.insert(diag.substr(
        kPrefix.size(), diag.find('\'', kPrefix.size()) - kPrefix.size()));
  }
  EXPECT_EQ(run.diags.size(), strays.size());
  EXPECT_EQ(strays,
            (std::multiset<std::string>{"Looped", "Named", "Nested", "Top"}));
}

TEST(InvokeParser, AChannelArrayIndexIsAConstantInsideTheArray) {
  // Hardware binds one fixed channel. A runtime index used to fold to
  // channel 0 and an index past the end to name no channel at all.
  constexpr char kCode[] = R"cpp(
    void Src(tapa::ostream<int>& out) {}
    void Snk(tapa::istream<int>& in) {}
    void Top(int k) {
      constexpr int kOne = 1;
      tapa::streams<int, 2> q;
      tapa::task()
          .invoke(Src, q[0])
          .invoke(Src, q[kOne])
          .invoke(Snk, q[k])
          .invoke(Snk, q[2])
          .invoke(Snk, q[0])
          .invoke(Snk, q[1]);
    }
  )cpp";
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/stream_index.cpp", kCode}}, "Top",
      testing::TempDir() + "/stream_index", {"-std=c++17"});
  ASSERT_EQ(run.diags.size(), 2u) << run.diags.front();
  for (const std::string& diag : run.diags) {
    EXPECT_NE(diag.find("needs an integer constant index inside the array"),
              std::string::npos)
        << diag;
  }
  const TaskModel* top = run.builder.FindTask("Top");
  ASSERT_NE(top, nullptr);
  EXPECT_EQ(top->instances.at("Src").at(1).args.at("out").arg, "q[1]");
}

TEST(InvokeParser, AnInvokeNamesItsTaskFunction) {
  // Through a function pointer the callee used to be skipped, so the task
  // silently dropped out of the graph.
  constexpr char kCode[] = R"cpp(
    void Leaf(tapa::mmap<int> m) {}
    constexpr auto kLeaf = &Leaf;
    void Top(tapa::mmap<int> a, tapa::mmap<int> b, tapa::mmap<int> c) {
      tapa::task().invoke(kLeaf, a).invoke(&Leaf, b).invoke(Leaf, c);
    }
  )cpp";
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/callee.cpp", kCode}}, "Top",
      testing::TempDir() + "/callee", {"-std=c++17"});
  ASSERT_EQ(run.diags.size(), 2u) << run.diags.front();
  for (const std::string& diag : run.diags) {
    EXPECT_NE(diag.find("must name its task function directly"),
              std::string::npos)
        << diag;
  }
}

TEST(InvokeParser, AnUpperTaskIsNotATemplate) {
  constexpr char kCode[] = R"cpp(
    template <int N>
    void Fill(tapa::ostream<int>& out) {}
    void Drain(tapa::istream<int>& in) {}
    template <int N>
    void Pipe(tapa::ostream<int>& out) {
      tapa::task().invoke(Fill<N>, out);
    }
    void Top() {
      tapa::stream<int> q;
      tapa::task().invoke(Pipe<4>, q).invoke(Drain, q);
    }
  )cpp";
  const test_support::TreePipelineRun run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/upper_template.cpp", kCode}}, "Top",
      testing::TempDir() + "/upper_template", {"-std=c++17"});
  ASSERT_FALSE(run.diags.empty());
  EXPECT_NE(run.diags.front().find("upper task 'Pipe' is a template"),
            std::string::npos)
      << run.diags.front();
}

TEST(InvokeParser, AnAsyncMmapGoesToOneTask) {
  // Software simulation would share one set of channels between the two
  // tasks; hardware gives each its own. A tapa::mmap may go to both.
  constexpr char kCode[] = R"cpp(
    void Worker(tapa::async_mmap<float>& mem) {}
    void Twice(tapa::async_mmap<float>& mem) {
      tapa::task().invoke(Worker, mem).invoke(Worker, mem);
    }
    void Shared(tapa::mmap<float> mem) {
      tapa::task().invoke(Worker, mem).invoke(Worker, mem);
    }
    void Top(tapa::mmap<float> a, tapa::mmap<float> b) {
      tapa::task().invoke(Twice, a).invoke(Shared, b);
    }
  )cpp";
  const auto run = test_support::RunTreePipeline(
      {test_support::VirtualTu{"/proj/handed_on.cpp", kCode}}, "Top",
      testing::TempDir() + "/handed_on", {"-std=c++17"});
  ASSERT_EQ(run.diags.size(), 1u);
  EXPECT_NE(run.diags.front().find("'mem' is handed to more than one task"),
            std::string::npos)
      << run.diags.front();
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
