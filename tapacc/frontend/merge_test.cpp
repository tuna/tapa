// Multi-TU merge rules (frontend/program_builder.h): each rule gets a
// synthetic two-"TU" program driven through the full
// index -> merge -> rewrite pipeline, the same way tapacc drives the builder
// over flattened TUs. The two TUs share nothing but the TAPA stub
// declarations, standing in for the shared inlined headers of flattened
// input.

#include "program_builder.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "nlohmann/json.hpp"

#include "clang/AST/ASTContext.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "program.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

constexpr char kTuA[] = "tu_a.cpp";
constexpr char kTuB[] = "tu_b.cpp";

std::unique_ptr<clang::ASTUnit> ParseTu(const std::string& file,
                                        const std::string& code) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      std::string(kTapaStubDecls) + "\n" + code,
      std::vector<std::string>{"-std=c++17"}, file);
  EXPECT_NE(ast, nullptr);
  return ast;
}

struct Merged {
  std::unique_ptr<clang::ASTUnit> a;
  std::unique_ptr<clang::ASTUnit> b;
  ProgramBuilder builder;

  Merged(std::unique_ptr<clang::ASTUnit> ast_a,
         std::unique_ptr<clang::ASTUnit> ast_b, ProgramBuilder built)
      : a(std::move(ast_a)), b(std::move(ast_b)), builder(std::move(built)) {}
};

// Build both TUs, index each, merge. `expect_ok` also runs the rewrite pass
// so code-level assertions work on the happy paths.
Merged Build(const std::string& code_a, const std::string& code_b,
             const std::string& top, bool expect_ok = true) {
  auto a = ParseTu(kTuA, code_a);
  auto b = ParseTu(kTuB, code_b);
  ProgramBuilder builder(top, SynthTarget::kXilinxHls);
  builder.IndexTu(a->getASTContext());
  builder.IndexTu(b->getASTContext());
  EXPECT_EQ(builder.MergeAndDiscover(), expect_ok);
  if (expect_ok) {
    builder.RewriteTu(a->getASTContext());
    builder.RewriteTu(b->getASTContext());
  }
  return Merged{std::move(a), std::move(b), std::move(builder)};
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
  auto m = Build(kCrossCaller, kCrossCallee, "Top");

  ASSERT_NE(m.builder.FindTask("Top"), nullptr);
  ASSERT_NE(m.builder.FindTask("Worker"), nullptr);
  EXPECT_EQ(m.builder.FindTask("Top")->level, TaskLevel::kUpper);
  EXPECT_EQ(m.builder.FindTask("Worker")->level, TaskLevel::kLower);

  // The invoking TU still parses the instances: Worker runs twice, wired
  // through the local FIFO.
  const TaskModel& top = *m.builder.FindTask("Top");
  EXPECT_EQ(top.instances.at("Worker").size(), 2u);
  ASSERT_EQ(top.streams.count("q"), 1u);

  // The rewritten text comes from the owning TU: Worker's blob carries TU
  // B's body and never sees the top, and the top's blob carries TU A's file
  // (shell body plus Worker's rewritten declaration).
  const std::string worker_code = m.builder.TaskCode("Worker");
  EXPECT_TRUE(Contains(worker_code, "out.write(in.read())"));
  EXPECT_FALSE(Contains(worker_code, "void Top("));
  const std::string top_code = m.builder.TaskCode("Top");
  EXPECT_TRUE(Contains(top_code, "void Top("));
  EXPECT_TRUE(Contains(top_code, "void Worker("));
}

TEST(Merge, InvokedTaskWithoutDefinitionAnywhereIsAnError) {
  auto m = Build(kCrossCaller, "", "Top", /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Worker' is invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, kTuA));
  EXPECT_TRUE(Contains(error, kTuB));
}

TEST(Merge, DeclDefSignatureMismatchNamesTheOtherDefinition) {
  // Same FQN, different parameter signature: the decl/def mismatch case.
  auto m = Build(kCrossCaller,
                 "void Worker(tapa::ostream<float>& out) {\n"
                 "  out.write(1.f);\n"
                 "}\n",
                 "Top", /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Worker' is invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, "different signature"));
  EXPECT_TRUE(Contains(error, kTuB));
}

// ── Rule 2: a header-defined task sighted by several TUs ───────────────

constexpr char kHeaderTask[] = R"cpp(
  void H(tapa::istream<float>& in, tapa::ostream<float>& out) {
    out.write(in.read());
  }
)cpp";

TEST(Merge, IdenticalSightingsDedupe) {
  // Both TUs carry the same definition (the flattened header) and the same
  // top: one merged task each, owned by the first TU in input order.
  const std::string top_body =
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(H, x, y);\n"
      "}\n";
  auto m = Build(kHeaderTask + top_body, kHeaderTask + top_body, "Top");
  EXPECT_EQ(m.builder.errors().size(), 0u);
  EXPECT_EQ(m.builder.EmitJson()["tasks"].size(), 2u);
  EXPECT_TRUE(Contains(m.builder.TaskCode("H"), "out.write(in.read())"));
}

TEST(Merge, ConflictingSightingsAreAnError) {
  // Same key (FQN + signature), different shape: TU A's H is a leaf, TU B's
  // H builds a tapa::task.
  const std::string top_a =
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(H, x, y);\n"
      "}\n";
  const std::string upper_h =
      "void Leaf(tapa::ostream<float>& out) {}\n"
      "void H(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  tapa::task().invoke(Leaf, out);\n"
      "}\n";
  auto m = Build(std::string(kHeaderTask) + top_a, upper_h, "Top",
                 /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'H' is defined differently"));
  EXPECT_TRUE(Contains(error, kTuA));
  EXPECT_TRUE(Contains(error, kTuB));
}

// ── Shared files: cross-TU helper definitions ──────────────────────────

// TU A owns the top and the helper that TU B's task calls; TU B owns the
// leaf task the top invokes. Both directions of the cross-TU boundary.
constexpr char kHelperOwnerTuA[] = R"cpp(
  float Half(float v) { return v / 2.f; }
  void Worker(tapa::istream<float>& in, tapa::ostream<float>& out);
  void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {
    tapa::stream<float> q;
    tapa::task().invoke(Worker, x, q).invoke(Worker, q, y);
  }
)cpp";

constexpr char kHelperUserTuB[] = R"cpp(
  float Half(float v);
  void Worker(tapa::istream<float>& in, tapa::ostream<float>& out) {
    for (int i = 0; i < 3; ++i) out.write(Half(in.read()));
  }
)cpp";

TEST(Merge, TaskSrcsListEveryOtherTusSharedFile) {
  // A task's manifest is its own blob plus every OTHER TU's shared file,
  // in input-file order; the owning TU's shared file is not its own src.
  auto m = Build(kHelperOwnerTuA, kHelperUserTuB, "Top");
  const nlohmann::json tasks = m.builder.EmitJson()["tasks"];
  ASSERT_EQ(tasks.size(), 2u);
  EXPECT_EQ(tasks.at("Top")["srcs"],
            nlohmann::json::array({"Top.cpp", "tu_b-shared.cpp"}));
  EXPECT_EQ(tasks.at("Worker")["srcs"],
            nlohmann::json::array({"Worker.cpp", "tu_a-shared.cpp"}));
}

TEST(Merge, SharedFileCarriesForeignHelperAndStubsEveryTask) {
  // TU A's shared file is TU A's text with no current task: Half keeps its
  // rewritten definition (what TU B's Worker blob only declares), while
  // every task — Top's own definition included — is a rewritten signature.
  auto m = Build(kHelperOwnerTuA, kHelperUserTuB, "Top");
  const std::string shared = m.builder.SharedCode(0);
  EXPECT_TRUE(Contains(shared, "return v / 2.f;"));
  EXPECT_TRUE(Contains(shared, "#pragma HLS inline off"));
  EXPECT_TRUE(Contains(shared, "void Top("));
  EXPECT_FALSE(Contains(shared, ".invoke("));
  EXPECT_FALSE(Contains(shared, "tapa::stream<float> q"));
  EXPECT_FALSE(Contains(shared, "out.write(Half(in.read())"));

  // TU B's shared file mirrors that shape: Worker stubbed, no helper
  // definition of its own to carry.
  const std::string shared_b = m.builder.SharedCode(1);
  EXPECT_TRUE(Contains(shared_b, "void Worker("));
  EXPECT_FALSE(Contains(shared_b, "out.write(Half(in.read())"));
}

TEST(Merge, DuplicateTuBasenamesAreAnError) {
  // Two TUs, distinct paths, one basename: no distinct shared file name
  // exists for them, so the merge refuses to run.
  auto a = ParseTu("d1/dup.cpp", kHelperOwnerTuA);
  auto b = ParseTu("d2/dup.cpp", kHelperUserTuB);
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls);
  builder.IndexTu(a->getASTContext());
  builder.IndexTu(b->getASTContext());
  EXPECT_FALSE(builder.MergeAndDiscover());
  const std::string error = OneError(builder);
  EXPECT_TRUE(Contains(error, "share the basename 'dup'"));
  EXPECT_TRUE(Contains(error, "d1/dup.cpp"));
  EXPECT_TRUE(Contains(error, "d2/dup.cpp"));
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
  // instantiates it too. One merged task, mangled key, wrapper after the
  // invoking top in the owning TU.
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
  auto m = Build(tu_a, tu_b, "Top");

  const nlohmann::json tasks = m.builder.EmitJson()["tasks"];
  ASSERT_EQ(tasks.size(), 2u);
  std::string spec_name;
  for (const auto& [name, task] : tasks.items()) {
    if (name != "Top") spec_name = name;
  }
  EXPECT_EQ(spec_name.rfind("tapa_mangled", 0), 0u);
  EXPECT_EQ(tasks.at(spec_name)["readable_name"], "Pass<float>");
  // The wrapper for the mangled entry point sits after the invoking top.
  EXPECT_TRUE(
      Contains(m.builder.TaskCode(spec_name), "void " + spec_name + "("));
}

// ── Rule 4: internal-linkage tasks are a hard error ────────────────────

TEST(Merge, InternalLinkageUpperTaskIsAnError) {
  auto m = Build(
      "template <typename T>\n"
      "void Pass(tapa::istream<T>& in, tapa::ostream<T>& out) {}\n"
      "static void Hidden(tapa::istream<float>& in) {\n"
      "  tapa::ostream<float> out;\n"
      "  tapa::task().invoke(Pass<float>, in, out);\n"
      "}\n"
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(Pass<float>, x, y);\n"
      "}\n",
      "", "Top", /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'Hidden'"));
  EXPECT_TRUE(Contains(error, "external linkage"));
}

TEST(Merge, InternalLinkageInvokedTaskIsAnError) {
  auto m = Build(
      "namespace {\n"
      "void SLeaf(tapa::istream<float>& in, tapa::ostream<float>& out) {}\n"
      "}\n"
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(SLeaf, x, y);\n"
      "}\n",
      "", "Top", /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "'SLeaf' invoked by 'Top'"));
  EXPECT_TRUE(Contains(error, "internal linkage"));
}

// ── Rule 5: the top ────────────────────────────────────────────────────

TEST(Merge, TopNotFoundListsEveryTu) {
  auto m = Build(kCrossCaller, kCrossCallee, "DoesNotExist",
                 /*expect_ok=*/false);
  const std::string error = OneError(m.builder);
  EXPECT_TRUE(Contains(error, "top-level task 'DoesNotExist' not found"));
  EXPECT_TRUE(Contains(error, kTuA));
  EXPECT_TRUE(Contains(error, kTuB));
}

TEST(Merge, TopSightedInSeveralTusIsTheHeaderDedupeCase) {
  // Identical top definitions in both TUs merge into one task (rule 2
  // applied to the top), not a redefinition.
  const std::string code =
      "void Leaf(tapa::istream<float>& in, tapa::ostream<float>& out) {}\n"
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task().invoke(Leaf, x, y);\n"
      "}\n";
  auto m = Build(code, code, "Top");
  EXPECT_TRUE(m.builder.errors().empty());
  EXPECT_EQ(m.builder.EmitJson()["tasks"].size(), 2u);
}

TEST(Merge, DistinctTopDefinitionsAreAnError) {
  // Same plain name, different signatures: two definitions of the top.
  auto m = Build(
      "void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {\n"
      "  tapa::task();\n"
      "}\n",
      "void Top(tapa::istream<float>& x) {\n"
      "  tapa::task();\n"
      "}\n",
      "Top", /*expect_ok=*/false);
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

TEST(Merge, SingleTuReachableSetAndLevels) {
  auto ast = ParseTu("single.cpp", kProgram);
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls);
  builder.IndexTu(ast->getASTContext());
  ASSERT_TRUE(builder.MergeAndDiscover());
  builder.RewriteTu(ast->getASTContext());

  // Top, Mid, Leaf, LeafIgn, TLeaf<float> — Unreached stays out and the
  // ignored upper's children stay out with it.
  EXPECT_EQ(builder.EmitJson()["tasks"].size(), 5u);
  EXPECT_NE(builder.FindTask("Top"), nullptr);
  EXPECT_NE(builder.FindTask("Mid"), nullptr);
  EXPECT_NE(builder.FindTask("Leaf"), nullptr);
  EXPECT_NE(builder.FindTask("LeafIgn"), nullptr);
  EXPECT_EQ(builder.FindTask("Unreached"), nullptr);
  EXPECT_EQ(builder.FindTask("Top")->level, TaskLevel::kUpper);
  EXPECT_EQ(builder.FindTask("Mid")->level, TaskLevel::kUpper);
  EXPECT_EQ(builder.FindTask("Leaf")->level, TaskLevel::kLower);
  // Ignored tasks are lower-level (their children are user-supplied RTL).
  EXPECT_EQ(builder.FindTask("LeafIgn")->level, TaskLevel::kLower);
  EXPECT_EQ(builder.FindTask("LeafIgn")->target, SynthTarget::kIgnore);
  EXPECT_EQ(builder.FindTask("Leaf")->target, SynthTarget::kXilinxHls);
}

TEST(Merge, SingleTuSrcsStayOneFilePerTask) {
  // No second TU means no shared file exists to reference: the manifest is
  // byte-identical to the pre-shared-file single-TU output.
  auto ast = ParseTu("single.cpp", kProgram);
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls);
  builder.IndexTu(ast->getASTContext());
  ASSERT_TRUE(builder.MergeAndDiscover());
  builder.RewriteTu(ast->getASTContext());

  const nlohmann::json tasks = builder.EmitJson()["tasks"];
  for (const auto& [name, task] : tasks.items()) {
    ASSERT_EQ(task["srcs"], nlohmann::json::array({name + ".cpp"}));
  }
}

TEST(Merge, SingleTuTemplateSpecialization) {
  auto ast = ParseTu("single.cpp", kProgram);
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls);
  builder.IndexTu(ast->getASTContext());
  ASSERT_TRUE(builder.MergeAndDiscover());
  builder.RewriteTu(ast->getASTContext());

  const nlohmann::json tasks = builder.EmitJson()["tasks"];
  for (const auto& [name, task] : tasks.items()) {
    if (name.rfind("tapa_mangled", 0) == 0) {
      EXPECT_EQ(task["readable_name"], "TLeaf<float>");
      EXPECT_EQ(task["level"], "lower");
      return;
    }
  }
  FAIL() << "no mangled template-specialization task in " << tasks.dump();
}

}  // namespace
}  // namespace tapa::cc
