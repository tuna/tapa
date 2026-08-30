// Tree-mode multi-TU merge rules (plan §3.3/§3.4): the same
// index -> merge -> rewrite pipeline merge_test.cpp drives, but over
// original (unflattened) sources with include logs, the way tapacc's
// `-tree` does. The two TUs share a real virtual header, so the tests can
// see the contracts only a single mirror can express: one definition site
// per task, per-task srcs naming every TU, no flatten-era shared files,
// and the shared header rewriting identically in every TU that includes
// it.

#include "program_builder.h"

#include <dirent.h>

#include <algorithm>
#include <functional>
#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "nlohmann/json.hpp"

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"

#include "codegen/tree_writer.h"
#include "program.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

using clang::tooling::FileContentMappings;

// A fresh writable directory per test, under the gtest temp root.
std::string TempRoot(const std::string& name) {
  const std::string root = testing::TempDir() + "/tree_merge_" + name;
  llvm::sys::fs::remove_directories(root);
  EXPECT_FALSE(llvm::sys::fs::create_directories(root));
  return root;
}

// Every file name under `root`, recursively, as "<dir>/<name>".
std::vector<std::string> WalkNames(const std::string& root) {
  std::vector<std::string> names;
  std::function<void(const std::string&, const std::string&)> walk =
      [&](const std::string& prefix, const std::string& dir) {
        DIR* stream = opendir(dir.c_str());
        ASSERT_NE(stream, nullptr);
        while (dirent* entry = readdir(stream)) {
          const std::string name = entry->d_name;
          if (name == "." || name == "..") continue;
          const std::string path = dir + "/" + name;
          if (entry->d_type == DT_DIR) {
            walk(prefix + name + "/", path);
          } else {
            names.push_back(prefix + name);
          }
        }
        closedir(stream);
      };
  walk("", root);
  return names;
}

std::string ReadFile(const std::string& path) {
  llvm::ErrorOr<std::unique_ptr<llvm::MemoryBuffer>> buffer =
      llvm::MemoryBuffer::getFile(path);
  return buffer ? (*buffer)->getBuffer().str() : std::string();
}

// Runs one pass of `builder` over the TU mapped at `main_file`, recording
// the include log first -- the shape of tapacc's BuilderAction, so the
// builder sees exactly what the `-tree` binary sees.
bool RunTreePass(ProgramBuilder* builder, bool index_pass,
                 const std::string& code, const std::string& main_file,
                 const std::vector<std::string>& args,
                 const FileContentMappings& virtual_files) {
  class PassAction : public clang::ASTFrontendAction {
   public:
    PassAction(ProgramBuilder* builder, bool index_pass)
        : builder_(builder), index_pass_(index_pass) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      ci.getPreprocessor().addPPCallbacks(
          std::make_unique<IncludeRecorder>(ci.getSourceManager(), &log_));
      return true;
    }

    std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
        clang::CompilerInstance&, llvm::StringRef) override {
      class Consumer : public clang::ASTConsumer {
       public:
        Consumer(ProgramBuilder* builder, bool index_pass,
                 std::vector<IncludeDirective>* log)
            : builder_(builder), index_pass_(index_pass), log_(log) {}

        void HandleTranslationUnit(clang::ASTContext& ctx) override {
          if (index_pass_) {
            builder_->IndexTu(ctx, log_);
          } else {
            builder_->RewriteTreeTu(ctx, std::move(*log_), /*tokens=*/nullptr);
          }
        }

       private:
        ProgramBuilder* builder_;
        bool index_pass_;
        std::vector<IncludeDirective>* log_;
      };
      return std::make_unique<Consumer>(builder_, index_pass_, &log_);
    }

   private:
    ProgramBuilder* builder_;
    bool index_pass_;
    std::vector<IncludeDirective> log_;
  };

  // The stub declarations ride in front of the TU's own text; the header
  // content stands alone so both TUs sight it at one virtual path.
  return clang::tooling::runToolOnCodeWithArgs(
      std::make_unique<PassAction>(builder, index_pass),
      std::string(kTapaStubDecls) + "\n" + code, args, main_file,
      "tree_merge_test", std::make_shared<clang::PCHContainerOperations>(),
      virtual_files);
}

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

struct TreeRun {
  std::string out_root;
  ProgramBuilder builder;
  bool ok = false;
};

// Index both TUs, merge, and run the tree rewrite pass over each, in the
// order tapacc runs them.
TreeRun RunTwoTuTree(const std::string& name) {
  const std::string root = TempRoot(name);
  TreeRun run{root,
              ProgramBuilder("Top", SynthTarget::kXilinxHls,
                             TreeConfig{root, {"/proj/a.cpp", "/proj/b.cpp"}}),
              false};
  const FileContentMappings files = FileContentMappings{
      {"/proj/shared.h", kSharedHeader},
  };
  const std::vector<std::string> args = {"-std=c++17", "-I/proj"};
  for (const bool index_pass : {true, false}) {
    EXPECT_TRUE(RunTreePass(&run.builder, index_pass, kTuA, "/proj/a.cpp", args,
                            files));
    EXPECT_TRUE(RunTreePass(&run.builder, index_pass, kTuB, "/proj/b.cpp", args,
                            files));
    if (index_pass) {
      EXPECT_TRUE(run.builder.MergeAndDiscover())
          << (run.builder.errors().empty() ? "" : run.builder.errors().front());
    }
  }
  std::string error;
  run.ok = run.builder.WriteTree(&error);
  EXPECT_TRUE(run.ok) << error;
  return run;
}

bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

TEST(TreeMerge, TwoTusMergeIntoOneGuardedTree) {
  const TreeRun run = RunTwoTuTree("twotu");

  // The merge itself: one task per definition, no diagnostics.
  ASSERT_TRUE(run.builder.errors().empty());
  const nlohmann::json tasks = run.builder.EmitJson()["tasks"];
  ASSERT_EQ(tasks.size(), 4u) << "Top, LeafA, LeafB and the Pass<float> spec";
  std::size_t specs = 0;
  for (const auto& [name, task] : tasks.items()) {
    specs += task["readable_name"] == "Pass<float>" ? 1 : 0;
  }
  EXPECT_EQ(specs, 1u);

  // The tree: both TUs and the shared header, each mirrored once.
  const std::vector<std::string> names = WalkNames(run.out_root);
  ASSERT_EQ(names.size(), 3u);
  for (const char* key : {"a.cpp", "b.cpp", "shared.h"}) {
    EXPECT_NE(std::find(names.begin(), names.end(), key), names.end())
        << "missing mirror of " << key << " in " << run.out_root;
  }

  // The shared header rewrote identically in both TUs: the template
  // primary carries the task guard even though only TU A instantiates it,
  // and the cross-TU declarations survive untouched.
  const std::string header = ReadFile(run.out_root + "/shared.h");
  EXPECT_TRUE(Contains(header, "#ifdef TAPA_TASK_DEF_"));
  EXPECT_TRUE(Contains(header, "void Top(tapa::istream<float>& in"));
  EXPECT_TRUE(Contains(header, "float Half(float v);"));
}

TEST(TreeMerge, TaskSrcsNameEveryTuIdentically) {
  const TreeRun run = RunTwoTuTree("srcs");
  const nlohmann::json tasks = run.builder.EmitJson()["tasks"];
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

TEST(TreeMerge, TreeModeEmitsNoFlattenSharedFiles) {
  // The flatten-only per-TU `-shared.cpp` scheme must not run in tree
  // mode: the whole tree compiles together, so a cross-TU helper travels
  // in its own mirrored TU.
  const TreeRun run = RunTwoTuTree("noshared");
  for (const std::string& name : WalkNames(run.out_root)) {
    EXPECT_EQ(name.find("-shared.cpp"), std::string::npos)
        << "tree mode wrote a flatten-era shared file: " << name;
  }
}

TEST(TreeMerge, DivergentHeaderRewriteFailsTheRewritePass) {
  // A header whose rewrite is context-dependent: the helper's stream depth
  // comes from a macro each TU defines differently, so the two TUs compute
  // different bytes for the one shared mirror. The second TU's rewrite
  // pass must fail (the §3.4 tripwire reports a hard diagnostic, which
  // fails the frontend action), while the first TU's pass still succeeds.
  const std::string root = TempRoot("diverge");
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls,
                         TreeConfig{root, {"/proj/a.cpp", "/proj/b.cpp"}});
  const std::string header =
      "void Top(tapa::istream<float>& in, tapa::ostream<float>& out);\n"
      "void Fill(tapa::ostream<float>& out) {\n"
      "  tapa::stream<float, DEPTH> s;\n"
      "  out.write(1.f);\n"
      "}\n";
  const FileContentMappings files = FileContentMappings{
      {"/proj/shared.h", header},
  };
  const std::vector<std::string> args = {"-std=c++17", "-I/proj"};
  const std::string top =
      "#include \"shared.h\"\n"
      "void Top(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  tapa::task().invoke(Fill, out);\n"
      "}\n";
  const std::string tu_a = "#define DEPTH 2\n" + top;
  const std::string tu_b = "#define DEPTH 3\n#include \"shared.h\"\n";
  ASSERT_TRUE(RunTreePass(&builder, /*index_pass=*/true, tu_a, "/proj/a.cpp",
                          args, files));
  ASSERT_TRUE(RunTreePass(&builder, /*index_pass=*/true, tu_b, "/proj/b.cpp",
                          args, files));
  EXPECT_TRUE(builder.MergeAndDiscover());
  EXPECT_TRUE(RunTreePass(&builder, /*index_pass=*/false, tu_a, "/proj/a.cpp",
                          args, files));
  EXPECT_FALSE(RunTreePass(&builder, /*index_pass=*/false, tu_b, "/proj/b.cpp",
                           args, files))
      << "a header rewritten differently per TU must fail the rewrite pass";
}

TEST(TreeMerge, SecondDefinitionSiteIsAnError) {
  // Same identity, two definition sites: one mirror cannot honor both.
  // TU A carries the composition's own LeafB definition, and TU B adds a
  // second one of its own instead of the leaf it normally owns.
  const std::string root = TempRoot("twosites");
  ProgramBuilder builder("Top", SynthTarget::kXilinxHls,
                         TreeConfig{root, {"/proj/a.cpp", "/proj/b.cpp"}});
  const FileContentMappings files = FileContentMappings{
      {"/proj/shared.h", kSharedHeader},
  };
  const std::vector<std::string> args = {"-std=c++17", "-I/proj"};
  const std::string leaf_b =
      "void LeafB(tapa::istream<float>& in, tapa::ostream<float>& out) {\n"
      "  out.write(in.read());\n"
      "}\n";
  const std::string tu_a = std::string(kTuA) + "\n" + leaf_b;
  const std::string tu_b = std::string("#include \"shared.h\"\n") + leaf_b;
  ASSERT_TRUE(RunTreePass(&builder, /*index_pass=*/true, tu_a, "/proj/a.cpp",
                          args, files));
  ASSERT_TRUE(RunTreePass(&builder, /*index_pass=*/true, tu_b, "/proj/b.cpp",
                          args, files));
  EXPECT_FALSE(builder.MergeAndDiscover());
  ASSERT_EQ(builder.errors().size(), 1u);
  const std::string& error = builder.errors().front();
  EXPECT_TRUE(Contains(error, "'LeafB' is defined at two locations"));
  EXPECT_TRUE(Contains(error, "/proj/a.cpp"));
  EXPECT_TRUE(Contains(error, "/proj/b.cpp"));
}

}  // namespace
}  // namespace tapa::cc
