// Multi-TU merge rules (frontend/program_builder.h): each rule gets a
// synthetic program driven through the full index -> merge -> rewrite ->
// tree-materialization pipeline, the way `tapacc` drives the builder over
// original sources. Two-TU fixtures share a real virtual header where the
// contract needs one (one definition site shared by several TUs);
// otherwise the TUs are independent virtual files. The pipeline harness is
// codegen/tree_pipeline.h.

#include <algorithm>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "nlohmann/json.hpp"

#include "codegen/tree_pipeline.h"

namespace tapa::cc {
namespace {

using test_support::TreePipelineRun;
using test_support::VirtualTu;

std::string TempRoot(const std::string& name) {
  return testing::TempDir() + "/merge_" + name;
}

// A two-TU run over independent files, both indexed before the merge.
TreePipelineRun RunTwoTus(const std::string& name, const std::string& code_a,
                          const std::string& code_b, const std::string& top) {
  return test_support::RunTreePipeline(
      {VirtualTu{"/proj/a.cpp", code_a}, VirtualTu{"/proj/b.cpp", code_b}}, top,
      TempRoot(name), {"-std=c++17"});
}

bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

std::string OneError(const ProgramBuilder& builder) {
  EXPECT_EQ(builder.errors().size(), 1u);
  return builder.errors().empty() ? std::string() : builder.errors().front();
}

// ── Rule 1: defined in one TU, invoked from another ────────────────────

// The top (TU A) sees only Worker's declaration; TU B owns the definition.
constexpr char kCrossCaller[] = R"cpp(
  void Worker(tapa::istream<float>& in, tapa::ostream<float>& out);

  void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {
    tapa::stream<float> q;
    tapa::task().invoke(Worker, x, q).invoke(Worker, q, y);
  }
)cpp";

constexpr char kCrossCallee[] = R"cpp(
  void Worker(tapa::istream<float>& in, tapa::ostream<float>& out) {
    for (int i = 0; i < 3; ++i) out.write(in.read());
  }
)cpp";

TEST(Merge, CrossTuInvokeTakesBodyFromOwningTu) {
  const TreePipelineRun m =
      RunTwoTus("cross", kCrossCaller, kCrossCallee, "Top");
  ASSERT_TRUE(m.ok) << (m.diags.empty() ? "" : m.diags.front());

  ASSERT_NE(m.builder.FindTask("Top"), nullptr);
  ASSERT_NE(m.builder.FindTask("Worker"), nullptr);
  EXPECT_EQ(m.builder.FindTask("Top")->level, TaskLevel::kUpper);
  EXPECT_EQ(m.builder.FindTask("Worker")->level, TaskLevel::kLower);

  // The invoking TU still parses the instances: Worker runs twice, wired
  // through the local FIFO.
  const TaskModel& top = *m.builder.FindTask("Top");
  EXPECT_EQ(top.instances.at("Worker").size(), 2u);
  ASSERT_EQ(top.streams.count("q"), 1u);

  // The definition renders once, in the owning TU's mirror, under its own
  // guard; the invoking TU carries only the declaration.
  ASSERT_EQ(m.files.count("a.cpp"), 1u);
  ASSERT_EQ(m.files.count("b.cpp"), 1u);
  EXPECT_TRUE(Contains(m.files.at("b.cpp"), "out.write(in.read())"));
  EXPECT_FALSE(Contains(m.files.at("a.cpp"), "out.write(in.read())"));
}

TEST(Merge, InvokedTaskWithoutDefinitionAnywhereIsAnError) {
  const TreePipelineRun m = RunTwoTus("nodef", kCrossCaller, "", "Top");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Worker' is invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, "/proj/a.cpp"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

TEST(Merge, DeclDefSignatureMismatchNamesTheOtherDefinition) {
  // Same FQN, different parameter signature: the decl/def mismatch case.
  const TreePipelineRun m =
      RunTwoTus("mismatch", kCrossCaller,
                "void Worker(tapa::ostream<float>& out) {\n"
                "  out.write(1.f);\n"
                "}\n",
                "Top");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Worker' is invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, "different signature"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

// ── Rule 2: one task, one definition site ──────────────────────────────

// A header task: several TUs include one definition.
constexpr char kHeaderH[] = R"cpp(
  void H(tapa::istream<float>& in, tapa::ostream<float>& out) {
    out.write(in.read());
  }
)cpp";

constexpr char kHeaderTop[] =
    "#include \"shared.h\"\n"
    "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
    "  tapa::task().invoke(H, x, y);\n"
    "}\n";

TEST(Merge, HeaderTaskSightedByBothTusMerges) {
  // Both TUs include the same header definition of H (TU B sights it
  // without defining anything of its own): one merged task, and the header
  // mirrors once with its body intact under H's guard.
  const TreePipelineRun m = test_support::RunTreePipeline(
      {VirtualTu{"/proj/a.cpp", kHeaderTop},
       VirtualTu{"/proj/b.cpp", "#include \"shared.h\"\n"}},
      "Top", TempRoot("header"), {"-std=c++17", "-I/proj"},
      {{"/proj/shared.h", kHeaderH}});
  EXPECT_TRUE(m.ok) << (m.diags.empty() ? "" : m.diags.front());
  EXPECT_EQ(m.builder.errors().size(), 0u);
  EXPECT_EQ(nlohmann::json::parse(m.json)["tasks"].size(), 2u);
  ASSERT_EQ(m.files.count("shared.h"), 1u);
  EXPECT_TRUE(Contains(m.files.at("shared.h"), "out.write(in.read())"));
}

TEST(Merge, ConflictingSightingsAreAnError) {
  // Same key (FQN + signature), different shape: TU A's H is a leaf, TU B's
  // H builds a tapa::task.
  const std::string top =
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(H, x, y);\n"
      "}\n";
  const std::string upper_h =
      "void Leaf(tapa::ostream<float>& out) {}\n"
      "void H(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  tapa::task().invoke(Leaf, out);\n"
      "}\n";
  const TreePipelineRun m =
      RunTwoTus("conflict", std::string(kHeaderH) + top, upper_h, "Top");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'H' is defined differently"));
  EXPECT_TRUE(Contains(error, "/proj/a.cpp"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

TEST(Merge, SecondDefinitionSiteIsAnError) {
  // Same identity, two definition sites: one mirror cannot honor both.
  // LeafB must be discovered (Top invokes it) for the site rule to fire.
  const std::string leaf_b =
      "void LeafB(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  out.write(in.read());\n"
      "}\n";
  const std::string top_a =
      "void LeafB(tapa::istream<float>& in, tapa::ostream<float>& out);\n"
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(LeafB, x, y);\n"
      "}\n";
  const TreePipelineRun m =
      RunTwoTus("twosites", top_a + leaf_b, leaf_b, "Top");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'LeafB' is defined at two locations"));
  EXPECT_TRUE(Contains(error, "/proj/a.cpp"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

// ── Rule 3: template specializations merge by mangled key ──────────────

constexpr char kTemplate[] = R"cpp(
  template <typename T>
  void Pass(tapa::istream<T>& in, tapa::ostream<T>& out) {
    out.write(in.read());
  }
)cpp";

TEST(Merge, TemplateSpecializationSightedInBothTusMerges) {
  // TU A's top instantiates Pass<float>; TU B's (unreached) upper task
  // instantiates it too. One merged task under its mangled key, whose
  // wrapper sits in the owning TU's mirror after the invoking top.
  const std::string tu_a = std::string(kTemplate) +
                           "void Top(tapa::istream<float>& x, "
                           "tapa::ostream<float>& y) {\n"
                           "  tapa::task().invoke(Pass<float>, x, y);\n"
                           "}\n";
  const std::string tu_b = std::string(kTemplate) +
                           "void Unused(tapa::istream<float>& x, "
                           "tapa::ostream<float>& y) {\n"
                           "  tapa::task().invoke(Pass<float>, x, y);\n"
                           "}\n";
  const TreePipelineRun m = RunTwoTus("template", tu_a, tu_b, "Top");
  ASSERT_TRUE(m.ok) << (m.diags.empty() ? "" : m.diags.front());

  const nlohmann::json tasks = nlohmann::json::parse(m.json)["tasks"];
  ASSERT_EQ(tasks.size(), 2u);
  std::string spec_name;
  for (const auto& [name, task] : tasks.items()) {
    if (name != "Top") spec_name = name;
  }
  EXPECT_EQ(spec_name.rfind("tapa_mangled", 0), 0u);
  EXPECT_EQ(tasks.at(spec_name)["readable_name"], "Pass<float>");
  // The wrapper for the mangled entry point sits after the invoking top,
  // inside the specialization's own guard.
  EXPECT_TRUE(Contains(m.files.at("a.cpp"), "void " + spec_name + "("))
      << m.files.at("a.cpp");
}

// ── Rule 4: internal-linkage tasks are a hard error ────────────────────

TEST(Merge, InternalLinkageUpperTaskIsAnError) {
  const TreePipelineRun m = test_support::RunTreePipeline(
      {VirtualTu{
          "/proj/a.cpp",
          "template <typename T>\n"
          "void Pass(tapa::istream<T>& in, tapa::ostream<T>& out) {}\n"
          "static void Hidden(tapa::istream<float>& in) {\n"
          "  tapa::ostream<float> out;\n"
          "  tapa::task().invoke(Pass<float>, in, out);\n"
          "}\n"
          "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
          "  tapa::task().invoke(Pass<float>, x, y);\n"
          "}\n"}},
      "Top", TempRoot("internal_upper"), {"-std=c++17"});
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Hidden'"));
  EXPECT_TRUE(Contains(error, "external linkage"));
}

TEST(Merge, InternalLinkageInvokedTaskIsAnError) {
  const TreePipelineRun m = test_support::RunTreePipeline(
      {VirtualTu{
          "/proj/a.cpp",
          "namespace {\n"
          "void SLeaf(tapa::istream<float>& in, "
          "tapa::ostream<float>& out) {}\n"
          "}\n"
          "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
          "  tapa::task().invoke(SLeaf, x, y);\n"
          "}\n"}},
      "Top", TempRoot("internal_invoked"), {"-std=c++17"});
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'SLeaf' invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, "internal linkage"));
}

// ── Rule 5: the top ────────────────────────────────────────────────────

TEST(Merge, TopNotFoundListsEveryTu) {
  const TreePipelineRun m =
      RunTwoTus("notop", kCrossCaller, kCrossCallee, "DoesNotExist");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "top-level task 'DoesNotExist' not found"));
  EXPECT_TRUE(Contains(error, "/proj/a.cpp"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

TEST(Merge, HeaderTopSightedBySeveralTusIsOneDefinition) {
  // The top defined in a header both TUs include: one definition site, one
  // merged task (the header dedupe case applied to the top).
  const std::string header =
      "void Leaf(tapa::istream<float>& in, tapa::ostream<float>& out) {}\n"
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(Leaf, x, y);\n"
      "}\n";
  const std::string tu = "#include \"shared.h\"\n";
  const TreePipelineRun m = test_support::RunTreePipeline(
      {VirtualTu{"/proj/a.cpp", tu}, VirtualTu{"/proj/b.cpp", tu}}, "Top",
      TempRoot("headertop"), {"-std=c++17", "-I/proj"},
      {{"/proj/shared.h", header}});
  EXPECT_TRUE(m.ok) << (m.diags.empty() ? "" : m.diags.front());
  EXPECT_TRUE(m.builder.errors().empty());
  EXPECT_EQ(nlohmann::json::parse(m.json)["tasks"].size(), 2u);
}

TEST(Merge, DistinctTopDefinitionsAreAnError) {
  // Same plain name, different signatures: two definitions of the top.
  const TreePipelineRun m =
      RunTwoTus("twotops",
                "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
                "  tapa::task();\n"
                "}\n",
                "void Top(tapa::istream<float>& x) {\n"
                "  tapa::task();\n"
                "}\n",
                "Top");
  EXPECT_FALSE(m.ok);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Top' has multiple definitions"));
}

// ── Single-TU discovery semantics carried over unchanged ───────────────

constexpr char kProgram[] = R"cpp(
  void Leaf(tapa::istream<float>& in, tapa::ostream<float>& out) {}
  [[tapa::target("ignore")]] void LeafIgn(tapa::istream<float>& in) {}
  [[tapa::target("ignore")]] void Unreached(tapa::istream<float>& in) {}
  template <typename T>
  void TLeaf(tapa::istream<T>& in) {}
  void Mid(tapa::istream<float>& a, tapa::ostream<float>& b) {
    tapa::task().invoke(Leaf, a, b);
  }
  void Top(tapa::istream<float>& x, tapa::istream<float>& w,
           tapa::istream<float>& z, tapa::ostream<float>& y) {
    tapa::stream<float> q;
    tapa::task()
        .invoke(Mid, x, q)
        .invoke(Leaf, q, y)
        .invoke(LeafIgn, w)
        .invoke(TLeaf<float>, z);
  }
)cpp";

TreePipelineRun BuildSingle() {
  auto run =
      test_support::RunTreePipeline({VirtualTu{"/proj/single.cpp", kProgram}},
                                    "Top", TempRoot("single"), {"-std=c++17"});
  EXPECT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  return run;
}

TEST(Merge, SingleTuReachableSetAndLevels) {
  const TreePipelineRun b = BuildSingle();

  // Top, Mid, Leaf, LeafIgn, TLeaf<float> — Unreached stays out and the
  // ignored upper's children stay out with it.
  EXPECT_EQ(nlohmann::json::parse(b.json)["tasks"].size(), 5u);
  EXPECT_NE(b.builder.FindTask("Top"), nullptr);
  EXPECT_NE(b.builder.FindTask("Mid"), nullptr);
  EXPECT_NE(b.builder.FindTask("Leaf"), nullptr);
  EXPECT_NE(b.builder.FindTask("LeafIgn"), nullptr);
  EXPECT_EQ(b.builder.FindTask("Unreached"), nullptr);
  EXPECT_EQ(b.builder.FindTask("Top")->level, TaskLevel::kUpper);
  EXPECT_EQ(b.builder.FindTask("Mid")->level, TaskLevel::kUpper);
  EXPECT_EQ(b.builder.FindTask("Leaf")->level, TaskLevel::kLower);
  // Ignored tasks are lower-level (their children are user-supplied RTL).
  EXPECT_EQ(b.builder.FindTask("LeafIgn")->level, TaskLevel::kLower);
  EXPECT_EQ(b.builder.FindTask("LeafIgn")->target, SynthTarget::kIgnore);
  EXPECT_EQ(b.builder.FindTask("Leaf")->target, SynthTarget::kXilinxHls);
}

TEST(Merge, SingleTuSrcsNameTheTuUnderOneIncludeRoot) {
  // Every task compiles the same mirrored TU; one define selects its
  // definition.
  const TreePipelineRun b = BuildSingle();
  const nlohmann::json tasks = nlohmann::json::parse(b.json)["tasks"];
  ASSERT_FALSE(tasks.empty());
  for (const auto& [name, task] : tasks.items()) {
    EXPECT_EQ(task["srcs"], nlohmann::json::array({"single.cpp"})) << name;
    EXPECT_EQ(task["include_dirs"], nlohmann::json::array({""})) << name;
  }
}

TEST(Merge, SingleTuTemplateSpecialization) {
  const TreePipelineRun b = BuildSingle();
  const nlohmann::json tasks = nlohmann::json::parse(b.json)["tasks"];
  for (const auto& [name, task] : tasks.items()) {
    if (name.rfind("tapa_mangled", 0) == 0) {
      EXPECT_EQ(task["readable_name"], "TLeaf<float>");
      EXPECT_EQ(task["level"], "lower");
      return;
    }
  }
  FAIL() << "no mangled template-specialization task in " << tasks.dump();
}

// ── The mirrored tree of a multi-TU composition ────────────────────────

// The composition under test: a shared header declaring every task and
// defining a template task, TU A owning the top and the helper TU B calls,
// TU B owning one leaf. Mirrors the shape of tests/apps/multi-file.
constexpr char kSharedHeader[] = R"cpp(
  void LeafA(tapa::istream<float>& in, tapa::ostream<float>& out);
  void LeafB(tapa::istream<float>& in, tapa::ostream<float>& out);
  void Top(tapa::istream<float>& in, tapa::ostream<float>& out);
  float Half(float v);
  template <typename T>
  void Pass(tapa::istream<T>& in, tapa::ostream<T>& out) {
    out.write(in.read());
  }
)cpp";

constexpr char kTuA[] = R"cpp(
#include "shared.h"
  float Half(float v) { return v / 2.f; }
  void LeafA(tapa::istream<float>& in, tapa::ostream<float>& out) {
    out.write(Half(in.read()));
  }
  void Top(tapa::istream<float>& in, tapa::ostream<float>& out) {
    tapa::stream<float> q, r;
    tapa::task()
        .invoke(LeafA, in, q)
        .invoke(LeafB, q, r)
        .invoke(Pass<float>, r, out);
  }
)cpp";

constexpr char kTuB[] = R"cpp(
#include "shared.h"
  void LeafB(tapa::istream<float>& in, tapa::ostream<float>& out) {
    out.write(Half(in.read()));
  }
)cpp";

TreePipelineRun RunComposition(const std::string& name) {
  return test_support::RunTreePipeline(
      {VirtualTu{"/proj/a.cpp", kTuA}, VirtualTu{"/proj/b.cpp", kTuB}}, "Top",
      TempRoot(name), {"-std=c++17", "-I/proj"},
      {{"/proj/shared.h", kSharedHeader}});
}

TEST(TreeMerge, TwoTusMergeIntoOneGuardedTree) {
  const TreePipelineRun run = RunComposition("twotu");
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());

  // The merge itself: one task per definition, no diagnostics.
  ASSERT_TRUE(run.builder.errors().empty());
  const nlohmann::json tasks = nlohmann::json::parse(run.json)["tasks"];
  ASSERT_EQ(tasks.size(), 4u) << "Top, LeafA, LeafB and the Pass<float> spec";
  std::size_t specs = 0;
  for (const auto& [name, task] : tasks.items()) {
    specs += task["readable_name"] == "Pass<float>" ? 1 : 0;
  }
  EXPECT_EQ(specs, 1u);

  // The tree: both TUs and the shared header, each mirrored once.
  ASSERT_EQ(run.files.size(), 3u);
  for (const char* key : {"a.cpp", "b.cpp", "shared.h"}) {
    EXPECT_NE(run.files.find(key), run.files.end())
        << "missing mirror of " << key;
  }

  // The shared header rewrote identically in both TUs: the template
  // primary carries the task guard even though only TU A instantiates it,
  // and the cross-TU declarations survive untouched.
  const std::string& header = run.files.at("shared.h");
  EXPECT_TRUE(Contains(header, "#ifdef TAPA_TASK_DEF_"));
  EXPECT_TRUE(Contains(header, "void Top(tapa::istream<float>& in"));
  EXPECT_TRUE(Contains(header, "float Half(float v);"));
}

TEST(TreeMerge, TaskSrcsNameEveryTuIdentically) {
  const TreePipelineRun run = RunComposition("srcs");
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const nlohmann::json tasks = nlohmann::json::parse(run.json)["tasks"];
  ASSERT_FALSE(tasks.empty());
  for (const auto& [name, task] : tasks.items()) {
    EXPECT_EQ(task["srcs"], nlohmann::json::array({"a.cpp", "b.cpp"}))
        << "task " << name;
    EXPECT_EQ(task["include_dirs"], nlohmann::json::array({""}))
        << "task " << name;
    const std::string define = task["defines"].at(0);
    EXPECT_NE(define.find("TAPA_TASK_DEF_"), std::string::npos) << name;
  }
}

TEST(TreeMerge, DivergentHeaderRewriteFailsTheRewritePass) {
  // A header whose rewrite is context-dependent: the helper's stream depth
  // comes from a macro each TU defines differently, so the two TUs compute
  // different bytes for the one shared mirror. The second TU's rewrite
  // pass reports the hard divergence diagnostic and no tree is written.
  const std::string header =
      "void Top(tapa::istream<float>& in, tapa::ostream<float>& out);\n"
      "void Fill(tapa::ostream<float>& out) {\n"
      "  tapa::stream<float, DEPTH> s;\n"
      "  out.write(1.f);\n"
      "}\n";
  const std::string top =
      "#include \"shared.h\"\n"
      "void Top(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  tapa::task().invoke(Fill, out);\n"
      "}\n";
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/a.cpp", "#define DEPTH 2\n" + top},
       VirtualTu{"/proj/b.cpp", "#define DEPTH 3\n#include \"shared.h\"\n"}},
      "Top", TempRoot("diverge"), {"-std=c++17", "-I/proj"},
      {{"/proj/shared.h", header}});
  EXPECT_FALSE(run.ok);
  ASSERT_EQ(run.diags.size(), 1u) << "the divergence must fail the rewrite";
  EXPECT_TRUE(Contains(run.diags.front(), "rewritten differently"))
      << run.diags.front();
  EXPECT_TRUE(run.files.empty()) << "a failed rewrite writes no tree";
}

}  // namespace
}  // namespace tapa::cc
