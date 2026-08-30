// Unit tests for the rewritten-tree writer (tree_writer.h): include
// recording, mirror layout, include-line rewriting, #line re-snap, and the
// write/skip/determinism contract. The multi-file compositions parse
// virtual sources mapped at absolute paths (the frontend-test pattern);
// each tree materializes under its own tempdir.

#include "tree_writer.h"

#include <dirent.h>

#include <functional>
#include <map>
#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/Path.h"
#include "llvm/Support/raw_ostream.h"

namespace tapa::cc {
namespace {

using clang::tooling::FileContentMappings;

// ── Test scaffolding ──────────────────────────────────────────────────────

// A fresh writable directory per test.
std::string TempRoot(const std::string& name) {
  const std::string root = testing::TempDir() + "/tree_writer_" + name;
  llvm::sys::fs::remove_directories(root);
  // Both calls report success as a null error code.
  EXPECT_FALSE(llvm::sys::fs::create_directories(root));
  return root;
}

std::string ReadFile(const std::string& path) {
  const auto buffer = llvm::MemoryBuffer::getFile(path);
  EXPECT_TRUE(buffer) << path;
  return buffer ? (*buffer)->getBuffer().str() : std::string();
}

// Every file under `root`, as "<relative path> -> <bytes>".
std::map<std::string, std::string> ReadTree(const std::string& root) {
  std::map<std::string, std::string> files;
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
            files[prefix + name] = ReadFile(path);
          }
        }
        closedir(stream);
      };
  walk("", root);
  return files;
}

// Parses one virtual TU (code mapped at `main_file`, extra virtual files,
// extra args) with the include recorder attached, and absorbs the log into
// `writer` -- inside the action, where the SourceManager is still alive.
// Returns the recorded log for assertions.
std::vector<IncludeDirective> ParseInto(
    TreeWriter* writer, const std::string& code, const std::string& main_file,
    const std::vector<std::string>& args,
    const FileContentMappings& virtual_files, bool* parsed) {
  class TreeAction : public clang::ASTFrontendAction {
   public:
    TreeAction(TreeWriter* writer, std::vector<IncludeDirective>* log,
               bool* absorbed)
        : writer_(writer), log_(log), absorbed_(absorbed) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      ci.getPreprocessor().addPPCallbacks(
          std::make_unique<IncludeRecorder>(ci.getSourceManager(), log_));
      return true;
    }

    std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
        clang::CompilerInstance&, llvm::StringRef) override {
      class Consumer : public clang::ASTConsumer {
       public:
        Consumer(TreeWriter* writer, std::vector<IncludeDirective>* log,
                 bool* absorbed)
            : writer_(writer), log_(log), absorbed_(absorbed) {}

        void HandleTranslationUnit(clang::ASTContext& ctx) override {
          std::string error;
          *absorbed_ = writer_->AddTu(ctx, *log_, &error);
          ASSERT_TRUE(*absorbed_) << error;
        }

       private:
        TreeWriter* writer_;
        std::vector<IncludeDirective>* log_;
        bool* absorbed_;
      };
      return std::make_unique<Consumer>(writer_, log_, absorbed_);
    }

   private:
    TreeWriter* writer_;
    std::vector<IncludeDirective>* log_;
    bool* absorbed_;
  };

  std::vector<IncludeDirective> log;
  bool absorbed = false;
  *parsed = clang::tooling::runToolOnCodeWithArgs(
      std::make_unique<TreeAction>(writer, &log, &absorbed), code, args,
      main_file, "tree_writer_test",
      std::make_shared<clang::PCHContainerOperations>(), virtual_files);
  EXPECT_TRUE(*parsed);
  EXPECT_TRUE(absorbed);
  return log;
}

// The multi-file composition shared by the writer-level tests: one TU
// including an in-root header (kept), an out-of-root header via -I
// (rewritten to its _external spelling), and a system header via -isystem
// (not mirrored at all).
constexpr char kMainCode[] = R"tapa(#include "keep.h"
#include "move.h"
#include <syshdr.h>
int main() { return kept() + moved(); }
)tapa";

struct Composition {
  std::vector<std::string> args;
  FileContentMappings virtual_files;
};

Composition MakeComposition() {
  Composition c;
  c.args = {"-std=c++17", "-I/w/vendor", "-isystem/w/sys"};
  c.virtual_files = FileContentMappings{
      {"/w/src/keep.h", "inline int kept() { return 1; }\n"},
      {"/w/vendor/move.h", "inline int moved() { return 2; }\n"},
      {"/w/sys/syshdr.h", "#define SYSHDR 0\n"},
  };
  return c;
}

// Runs the composition into `out_root`: parse + absorb, then the
// byte-level Write() over plain data.
struct RunResult {
  bool ok = false;
  std::string error;
  std::vector<IncludeDirective> log;
};

RunResult RunComposition(const std::string& out_root) {
  const Composition c = MakeComposition();
  TreeWriter writer(out_root, {"/w/src/main.cpp"});
  RunResult result;
  bool parsed = false;
  result.log = ParseInto(&writer, kMainCode, "/w/src/main.cpp", c.args,
                         c.virtual_files, &parsed);
  result.ok = parsed && writer.Write(&result.error);
  return result;
}

// ── (a) src-root computation ──────────────────────────────────────────────

TEST(TreeLayout, SrcRootIsDeepestCommonAncestor) {
  EXPECT_EQ(TreeLayout::SrcRoot({"/a/b/main.cpp"}), "/a/b");
  EXPECT_EQ(TreeLayout::SrcRoot({"/a/b/main.cpp", "/a/other.cpp"}), "/a");
  // Nested dirs: the root stops at the shared directory, not deeper.
  EXPECT_EQ(TreeLayout::SrcRoot({"/a/b/c/m.cpp", "/a/b/n.cpp"}), "/a/b");
  EXPECT_EQ(TreeLayout::SrcRoot({"/main.cpp"}), "/");
}

TEST(TreeLayout, InRootHonorsComponentBoundaries) {
  const TreeLayout layout("/a");
  EXPECT_TRUE(layout.InRoot("/a/x.cpp"));
  EXPECT_TRUE(layout.InRoot("/a/b/x.cpp"));
  // "/ab" shares the string prefix but not the directory.
  EXPECT_FALSE(layout.InRoot("/ab/x.cpp"));
  EXPECT_FALSE(layout.InRoot("/a"));
  EXPECT_EQ(layout.InRootKey("/a/b/x.cpp"), "b/x.cpp");

  const TreeLayout fs_root("/");
  EXPECT_TRUE(fs_root.InRoot("/x.cpp"));
  EXPECT_EQ(fs_root.InRootKey("/x.cpp"), "x.cpp");
}

// ── (b) _external buckets and collision ───────────────────────────────────

TEST(TreeLayout, OutOfRootFilesLandInDigestBuckets) {
  // sha256("/opt/vendor/util.h") starts c3f5a749 (lowercase hex).
  EXPECT_EQ(TreeLayout::ExternalKey("/opt/vendor/util.h"),
            "_external/c3f5a749/util.h");
  // Same path, same bucket; distinct paths, distinct buckets.
  EXPECT_EQ(TreeLayout::ExternalKey("/opt/vendor/util.h"),
            TreeLayout::ExternalKey("/opt/vendor/util.h"));
  EXPECT_NE(TreeLayout::ExternalKey("/opt/vendor/util.h"),
            TreeLayout::ExternalKey("/opt/vendor2/util.h"));
}

TEST(TreeLayout, KeyCollisionIsAHardError) {
  TreeLayout layout("/root");
  std::string key;
  std::string error;
  ASSERT_TRUE(layout.Register("/root/a.cpp", &key, &error));
  EXPECT_EQ(key, "a.cpp");
  // Re-registering the same path is the same fact, not a collision.
  EXPECT_TRUE(layout.Register("/root/a.cpp", &key, &error));

  // An out-of-root file's bucket can be occupied by an in-root file the
  // user happens to keep under _external/: two files, one tree path.
  const std::string outside = "/elsewhere/foo.h";
  ASSERT_TRUE(layout.Register(outside, &key, &error));
  const std::string squatter = "/root/" + key;
  EXPECT_FALSE(layout.Register(squatter, &key, &error));
  EXPECT_NE(error.find("two distinct files"), std::string::npos);
  EXPECT_NE(error.find(outside), std::string::npos);
  EXPECT_NE(error.find(squatter), std::string::npos);
}

// ── Recording + rewriting ─────────────────────────────────────────────────

TEST(IncludeRecorder, RecordsResolutionKindAndSpelling) {
  const RunResult run = RunComposition(TempRoot("records"));
  ASSERT_TRUE(run.ok) << run.error;
  ASSERT_EQ(run.log.size(), 3u);

  const IncludeDirective& keep = run.log[0];
  EXPECT_EQ(keep.writing_file, "/w/src/main.cpp");
  EXPECT_EQ(keep.spelling, "keep.h");
  EXPECT_FALSE(keep.angled);
  EXPECT_FALSE(keep.from_system_search);
  EXPECT_EQ(keep.resolved, "/w/src/keep.h");

  const IncludeDirective& move = run.log[1];
  EXPECT_EQ(move.spelling, "move.h");
  EXPECT_FALSE(move.angled);
  EXPECT_FALSE(move.from_system_search);
  EXPECT_EQ(move.resolved, "/w/vendor/move.h");

  const IncludeDirective& sys = run.log[2];
  EXPECT_EQ(sys.spelling, "syshdr.h");
  EXPECT_TRUE(sys.angled);
  EXPECT_TRUE(sys.from_system_search);
  EXPECT_EQ(sys.resolved, "/w/sys/syshdr.h");
}

TEST(TreeWriter, MirrorsTheTreeAndRewritesOnlyMovedIncludes) {
  const std::string root = TempRoot("mirror");
  ASSERT_TRUE(RunComposition(root).ok);

  const std::map<std::string, std::string> files = ReadTree(root);
  ASSERT_EQ(files.size(), 3u) << "main + keep.h + one bucketed header";
  EXPECT_NE(files.find("main.cpp"), files.end());
  EXPECT_NE(files.find("keep.h"), files.end());
  // The system header is not mirrored.
  EXPECT_EQ(files.find("syshdr.h"), files.end());

  // Exactly one _external bucket, named by the digest of the moved path.
  const std::string bucket = "_external/4dc19a3d";
  ASSERT_NE(files.find(bucket + "/move.h"), files.end());
  EXPECT_EQ(files.at(bucket + "/move.h"), "inline int moved() { return 2; }\n");
  EXPECT_EQ(files.at("keep.h"), "inline int kept() { return 1; }\n");

  // The quoted include kept its in-root spelling; the moved one points at
  // its bucket; the angle include is untouched.
  EXPECT_EQ(files.at("main.cpp"),
            "#include \"keep.h\"\n#include \"" + bucket +
                "/move.h\"\n#include <syshdr.h>\nint main() { return kept() + "
                "moved(); }\n");
}

// ── (d) #line re-snap ─────────────────────────────────────────────────────

// clang-format formats the contents of R"cpp(...)" literals; these
// fixtures are byte-exact inputs, so they use a delimiter it leaves alone.
constexpr char kLinesCode[] = R"tapa(int one;
void f() {
  one = 1;
}
int two;
)tapa";

// A char range over [offset, offset + length) of the main file.
clang::CharSourceRange RangeOf(clang::SourceManager& sm, size_t offset,
                               size_t length) {
  const clang::SourceLocation begin =
      sm.getLocForStartOfFile(sm.getMainFileID()).getLocWithOffset(offset);
  return clang::CharSourceRange::getCharRange(begin,
                                              begin.getLocWithOffset(length));
}

TEST(TreeFileBuffer, MultiLineReplacementResnapsWithLineMarker) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  clang::Rewriter rewriter(sm, ast->getLangOpts());
  TreeFileBuffer buffer(sm, rewriter, sm.getMainFileID(), "/src/main.cpp");

  // Replace f's 3-line definition with a 4-line one (+1 line): the edit
  // itself shifts `int two;` down, so a marker must pin it back to line 5.
  const std::string body = "void f() {\n  one = 1;\n}";
  const size_t offset = std::string(kLinesCode).find(body);
  ASSERT_NE(offset, std::string::npos);
  ASSERT_TRUE(
      buffer.ReplaceWithLineResnap(RangeOf(sm, offset, body.size()),
                                   "void f() {\n  one = 1;\n  one += 1;\n}"));
  EXPECT_TRUE(buffer.had_edits());
  // Exact bytes: the marker names the original file and the original line
  // of the text following the edit.
  EXPECT_EQ(buffer.Render(),
            "int one;\n"
            "void f() {\n"
            "  one = 1;\n"
            "  one += 1;\n"
            "}\n"
            "#line 5 \"/src/main.cpp\"\n"
            "int two;\n");
}

TEST(TreeSession, DeclEditSinkResnapsInsideTheFile) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  TreeSession session(ast->getASTContext(), /*log=*/{}, {"main.cpp"},
                      /*tokens=*/nullptr);
  session.edits().Describe("function 'f'");

  const size_t brace_offset = std::string(kLinesCode).find("void f() {") + 9;
  const clang::SourceLocation brace =
      sm.getLocForStartOfFile(sm.getMainFileID())
          .getLocWithOffset(brace_offset);
  ASSERT_FALSE(
      session.edits().InsertTextAfterToken(brace, "\n  int inserted;"));

  TreeFileBuffer* const buffer = session.BufferForPath("main.cpp");
  ASSERT_NE(buffer, nullptr);
  EXPECT_EQ(buffer->Render(),
            "int one;\n"
            "void f() {\n"
            "  int inserted;\n"
            "#line 3 \"main.cpp\"\n"
            "  one = 1;\n"
            "}\n"
            "int two;\n");
}

TEST(TreeFileBuffer, GuardWrappingResnapsBothHalves) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  clang::Rewriter rewriter(sm, ast->getLangOpts());
  TreeFileBuffer buffer(sm, rewriter, sm.getMainFileID(), "/src/main.cpp");

  const std::string body = "void f() {\n  one = 1;\n}";
  const size_t offset = std::string(kLinesCode).find(body);
  ASSERT_NE(offset, std::string::npos);
  const clang::SourceLocation begin =
      sm.getLocForStartOfFile(sm.getMainFileID()).getLocWithOffset(offset);
  // A SourceRange end names the START of the last token (the closing
  // brace), the way AST ranges do; InsertGuard extends past it itself.
  const clang::SourceLocation last_token =
      begin.getLocWithOffset(body.size() - 1);
  ASSERT_TRUE(buffer.InsertGuard(clang::SourceRange(begin, last_token),
                                 "#ifdef TAPA_TASK_DEF_F\n",
                                 "\n#else\nvoid f();\n#endif\n"));
  // The closing text ends the `}` line; the original newline that
  // followed it then reads as a blank line before the marker (still
  // logically line 4), and `int two;` re-snaps to its original line 5.
  EXPECT_EQ(buffer.Render(),
            "int one;\n"
            "#ifdef TAPA_TASK_DEF_F\n"
            "#line 2 \"/src/main.cpp\"\n"
            "void f() {\n"
            "  one = 1;\n"
            "}\n"
            "#else\n"
            "void f();\n"
            "#endif\n"
            "\n"
            "#line 5 \"/src/main.cpp\"\n"
            "int two;\n");
}

TEST(TreeFileBuffer, LaterEditEarlierInTheFileStillResnaps) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  clang::Rewriter rewriter(sm, ast->getLangOpts());
  TreeFileBuffer buffer(sm, rewriter, sm.getMainFileID(), "/src/main.cpp");

  // Edit the later region first, then the earlier one: both markers are
  // computed in original coordinates, so each pins the text that follows
  // it to that text's original line no matter the application order.
  const std::string body = "void f() {\n  one = 1;\n}";
  const size_t body_offset = std::string(kLinesCode).find(body);
  ASSERT_NE(body_offset, std::string::npos);
  ASSERT_TRUE(
      buffer.ReplaceWithLineResnap(RangeOf(sm, body_offset, body.size()),
                                   "void f() {\n  one = 1;\n  one += 1;\n}"));
  ASSERT_TRUE(
      buffer.ReplaceWithLineResnap(RangeOf(sm, 0, 8), "int one;\nint one_b;"));
  EXPECT_EQ(buffer.Render(),
            "int one;\n"
            "int one_b;\n"
            "#line 2 \"/src/main.cpp\"\n"
            "void f() {\n"
            "  one = 1;\n"
            "  one += 1;\n"
            "}\n"
            "#line 5 \"/src/main.cpp\"\n"
            "int two;\n");
}

TEST(TreeFileBuffer, LineCountPreservingEditsEmitNoMarker) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  clang::Rewriter rewriter(sm, ast->getLangOpts());
  TreeFileBuffer buffer(sm, rewriter, sm.getMainFileID(), "/src/main.cpp");

  const size_t offset = std::string(kLinesCode).find("one = 1");
  ASSERT_NE(offset, std::string::npos);
  ASSERT_TRUE(buffer.ReplaceWithLineResnap(RangeOf(sm, offset, 7), "one = 2"));
  EXPECT_TRUE(buffer.had_edits());
  EXPECT_EQ(buffer.Render(),
            "int one;\n"
            "void f() {\n"
            "  one = 2;\n"
            "}\n"
            "int two;\n");
}

TEST(TreeFileBuffer, UneditedBuffersRenderByteIdentical) {
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      kLinesCode, std::vector<std::string>{"-std=c++17"}, "main.cpp");
  ASSERT_NE(ast, nullptr);
  clang::SourceManager& sm = ast->getSourceManager();
  clang::Rewriter rewriter(sm, ast->getLangOpts());
  TreeFileBuffer buffer(sm, rewriter, sm.getMainFileID(), "/src/main.cpp");
  EXPECT_FALSE(buffer.had_edits());
  EXPECT_EQ(buffer.Render(), kLinesCode);
}

// ── (e) same-content skip ─────────────────────────────────────────────────

TEST(TreeWriter, UnchangedBytesAreNotRewritten) {
  const std::string root = TempRoot("skip");
  ASSERT_TRUE(RunComposition(root).ok);

  // Make every written file read-only: a skipped file is never opened for
  // writing, so the second Write succeeds only if every byte matched.
  for (const auto& [path, content] : ReadTree(root)) {
    (void)content;
    EXPECT_FALSE(llvm::sys::fs::setPermissions(
        root + "/" + path,
        llvm::sys::fs::perms::all_read & ~llvm::sys::fs::perms::all_write));
  }
  const RunResult again = RunComposition(root);
  EXPECT_TRUE(again.ok) << again.error;

  // The skip is a byte comparison: changed on-disk content gets restored.
  EXPECT_FALSE(llvm::sys::fs::setPermissions(root + "/main.cpp",
                                             llvm::sys::fs::perms::all_all));
  {
    std::error_code ec;
    llvm::raw_fd_ostream corrupt(root + "/main.cpp", ec);
    ASSERT_FALSE(ec) << ec.message();
    corrupt << "garbage\n";
  }
  const RunResult restore = RunComposition(root);
  EXPECT_TRUE(restore.ok) << restore.error;
  EXPECT_EQ(ReadFile(root + "/main.cpp"),
            "#include \"keep.h\"\n#include \"_external/4dc19a3d/move.h\"\n"
            "#include <syshdr.h>\nint main() { return kept() + moved(); }\n");
}

// ── (f) determinism ───────────────────────────────────────────────────────

TEST(TreeWriter, IdenticalInputsProduceIdenticalTrees) {
  const std::string root_a = TempRoot("det_a");
  const std::string root_b = TempRoot("det_b");
  ASSERT_TRUE(RunComposition(root_a).ok);
  ASSERT_TRUE(RunComposition(root_b).ok);
  EXPECT_EQ(ReadTree(root_a), ReadTree(root_b));

  // Rewriting into an already-written root is a no-op by bytes...
  ASSERT_TRUE(RunComposition(root_a).ok);
  EXPECT_EQ(ReadTree(root_a), ReadTree(root_b));
}

// ── multi-TU ownership ────────────────────────────────────────────────────

// Parses one TU, rewrites the shared header's helper name through the
// session's edit sink (the surface the decl-rewrite rules write through),
// then absorbs the TU: the shape of one tree-mode rewrite pass, with the
// replacement standing for whatever the real rewrites compute per TU.
bool AbsorbRenamingTu(TreeWriter* writer, const std::string& code,
                      const std::string& main_file,
                      const std::vector<std::string>& args,
                      const FileContentMappings& virtual_files,
                      llvm::StringRef replacement, std::string* error) {
  class RenameAction : public clang::ASTFrontendAction {
   public:
    RenameAction(TreeWriter* writer, llvm::StringRef replacement,
                 std::string* error)
        : writer_(writer), replacement_(replacement), error_(error) {}

    bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
      ci.getPreprocessor().addPPCallbacks(
          std::make_unique<IncludeRecorder>(ci.getSourceManager(), &log_));
      return true;
    }

    std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
        clang::CompilerInstance&, llvm::StringRef) override {
      class Consumer : public clang::ASTConsumer {
       public:
        Consumer(RenameAction* owner) : owner_(owner) {}
        void HandleTranslationUnit(clang::ASTContext& ctx) override {
          TreeSession session(ctx, owner_->log_,
                              {"/proj/src/a.cpp", "/proj/src/sub/b.cpp"},
                              /*tokens=*/nullptr);
          // The header-defined helper, located through the AST the way a
          // decl-rewrite rule would.
          const clang::FunctionDecl* helper = nullptr;
          class Find : public clang::RecursiveASTVisitor<Find> {
           public:
            Find(const clang::FunctionDecl** out) : out_(out) {}
            bool VisitFunctionDecl(clang::FunctionDecl* decl) {
              if (decl->getNameAsString() == "shared") *out_ = decl;
              return true;
            }
            const clang::FunctionDecl** out_;
          };
          Find find(&helper);
          find.TraverseDecl(ctx.getTranslationUnitDecl());
          ASSERT_NE(helper, nullptr) << "shared.h must define 'shared'";
          session.edits().Describe("helper 'shared'");
          // The Rewriter convention: true means the edit was rejected.
          if (session.edits().ReplaceText(
                  helper->getLocation(),
                  static_cast<unsigned>(llvm::StringRef("shared").size()),
                  owner_->replacement_)) {
            *owner_->error_ = "the test edit was rejected";
            owner_->absorbed_ = false;
            return;
          }
          std::string error;
          owner_->absorbed_ = owner_->writer_->AddTu(
              ctx, std::move(owner_->log_), session, &error);
          if (!owner_->absorbed_) *owner_->error_ = error;
        }

       private:
        RenameAction* owner_;
      };
      return std::make_unique<Consumer>(this);
    }

    bool absorbed() const { return absorbed_; }

   private:
    TreeWriter* writer_;
    llvm::StringRef replacement_;
    std::string* error_;
    bool absorbed_ = false;
    std::vector<IncludeDirective> log_;
  };

  auto action = std::make_unique<RenameAction>(writer, replacement, error);
  RenameAction* const handle = action.get();
  const bool parsed = clang::tooling::runToolOnCodeWithArgs(
      std::move(action), code, args, main_file, "tree_writer_test",
      std::make_shared<clang::PCHContainerOperations>(), virtual_files);
  return parsed && handle->absorbed();
}

TEST(TreeWriter, TwoTusShareTheTreeAndTheirHeaders) {
  // Two TUs under one src root, both including the shared out-of-root
  // header and each their own in-root header. Both TUs' logs feed one
  // writer; the shared headers land once, referenced by both TUs.
  const std::string root = TempRoot("twotus");
  TreeWriter writer(root, {"/proj/src/a.cpp", "/proj/src/sub/b.cpp"});
  const FileContentMappings files = FileContentMappings{
      {"/w/vendor/move.h", "inline int moved() { return 2; }\n"},
      {"/proj/src/a_helper.h", "inline int a() { return 1; }\n"},
      {"/proj/src/sub/b_helper.h", "inline int b() { return 2; }\n"},
  };
  const std::vector<std::string> args = {"-std=c++17", "-I/w/vendor",
                                         "-I/proj/src"};

  bool parsed = false;
  ParseInto(&writer,
            "#include \"a_helper.h\"\n#include \"move.h\"\n"
            "int A() { return a() + moved(); }\n",
            "/proj/src/a.cpp", args, files, &parsed);
  ASSERT_TRUE(parsed);
  ParseInto(
      &writer,
      "#include \"b_helper.h\"\n#include \"a_helper.h\"\n#include \"move.h\"\n"
      "int B() { return b() + a() + moved(); }\n",
      "/proj/src/sub/b.cpp", args, files, &parsed);
  ASSERT_TRUE(parsed);

  std::string error;
  ASSERT_TRUE(writer.Write(&error)) << error;

  const std::map<std::string, std::string> tree = ReadTree(root);
  EXPECT_EQ(tree.size(), 5u) << "two TUs, two helpers, one shared bucket";
  EXPECT_NE(tree.find("a.cpp"), tree.end());
  EXPECT_NE(tree.find("sub/b.cpp"), tree.end());
  EXPECT_NE(tree.find("a_helper.h"), tree.end());
  EXPECT_NE(tree.find("sub/b_helper.h"), tree.end());
  const std::string bucket = "_external/4dc19a3d/move.h";
  ASSERT_NE(tree.find(bucket), tree.end());
  // Both TUs reference the bucket; in-root spellings keep resolving in
  // the mirror, so they are untouched.
  EXPECT_EQ(tree.at("a.cpp"), "#include \"a_helper.h\"\n#include \"" + bucket +
                                  "\"\n"
                                  "int A() { return a() + moved(); }\n");
  EXPECT_EQ(tree.at("sub/b.cpp"),
            "#include \"b_helper.h\"\n#include \"a_helper.h\"\n"
            "#include \"" +
                bucket +
                "\"\n"
                "int B() { return b() + a() + moved(); }\n");
}

// ── per-TU byte-identity tripwire (plan §3.4) ─────────────────────────────

// Two TUs, one shared in-root header defining a helper. The src root is
// /proj/src, so the header mirrors at "shared.h" and both TUs include it.
constexpr char kSharedHelper[] = "int shared() { return 7; }\n";

struct TuComposition {
  FileContentMappings files;
  std::vector<std::string> args;
  std::string tu_a;
  std::string tu_b;
};

TuComposition SharedHeaderComposition() {
  TuComposition c;
  c.files = FileContentMappings{
      {"/proj/src/shared.h", kSharedHelper},
  };
  c.args = {"-std=c++17", "-I/proj/src"};
  c.tu_a = "#include \"shared.h\"\nint A() { return shared(); }\n";
  c.tu_b = "#include \"shared.h\"\nint B() { return shared() + 1; }\n";
  return c;
}

TEST(TreeWriter, DivergentHeaderRewriteIsAHardError) {
  // The two TUs rewrite the shared header differently (the stand-in for a
  // context-dependent rewrite, e.g. a macro expanding differently per TU):
  // one mirror serves every task variant, so that has no single honest
  // rendering and the second TU's absorption fails, naming both TUs.
  const TuComposition c = SharedHeaderComposition();
  const std::string root = TempRoot("diverge");
  TreeWriter writer(root, {"/proj/src/a.cpp", "/proj/src/sub/b.cpp"});
  std::string error;
  ASSERT_TRUE(AbsorbRenamingTu(&writer, c.tu_a, "/proj/src/a.cpp", c.args,
                               c.files, "shared_a", &error))
      << error;
  EXPECT_FALSE(AbsorbRenamingTu(&writer, c.tu_b, "/proj/src/sub/b.cpp", c.args,
                                c.files, "shared_b", &error));
  EXPECT_NE(error.find("'/proj/src/shared.h'"), std::string::npos) << error;
  EXPECT_NE(error.find("'/proj/src/a.cpp'"), std::string::npos) << error;
  EXPECT_NE(error.find("'/proj/src/sub/b.cpp'"), std::string::npos) << error;
  EXPECT_NE(error.find("rewrite identically"), std::string::npos) << error;
}

TEST(TreeWriter, IdenticalHeaderRewritesMergeSilently) {
  // Same header, same rewrite in both TUs: the first sighting owns the
  // bytes, the second agrees, and the tree carries one edited header.
  const TuComposition c = SharedHeaderComposition();
  const std::string root = TempRoot("converge");
  TreeWriter writer(root, {"/proj/src/a.cpp", "/proj/src/sub/b.cpp"});
  std::string error;
  ASSERT_TRUE(AbsorbRenamingTu(&writer, c.tu_a, "/proj/src/a.cpp", c.args,
                               c.files, "shared_renamed", &error))
      << error;
  ASSERT_TRUE(AbsorbRenamingTu(&writer, c.tu_b, "/proj/src/sub/b.cpp", c.args,
                               c.files, "shared_renamed", &error))
      << error;
  ASSERT_TRUE(writer.Write(&error)) << error;
  const std::map<std::string, std::string> tree = ReadTree(root);
  ASSERT_EQ(tree.size(), 3u);
  EXPECT_EQ(tree.at("shared.h"), "int shared_renamed() { return 7; }\n");
}

}  // namespace
}  // namespace tapa::cc
