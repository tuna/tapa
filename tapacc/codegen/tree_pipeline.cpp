#include "tree_pipeline.h"

#include <dirent.h>

#include <functional>
#include <memory>

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"

#include "diag_capture.h"
#include "frontend/tapa_stub_decls.h"
#include "macro_splice.h"
#include "tree_writer.h"

namespace tapa::cc::test_support {
namespace {

std::string ReadFile(const std::string& path) {
  const auto buffer = llvm::MemoryBuffer::getFile(path);
  return buffer ? (*buffer)->getBuffer().str() : std::string();
}

std::map<std::string, std::string> ReadTree(const std::string& root) {
  std::map<std::string, std::string> files;
  std::function<void(const std::string&, const std::string&)> walk =
      [&](const std::string& prefix, const std::string& dir) {
        DIR* stream = opendir(dir.c_str());
        if (stream == nullptr) return;
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

// One frontend action over one TU, the exact shape of tapacc's
// BuilderAction: the include log is always recorded, and only the rewrite
// pass records the expanded-token stream (only it edits).
class PipelineAction : public clang::ASTFrontendAction {
 public:
  PipelineAction(ProgramBuilder& builder, bool index_pass,
                 CollectingDiagConsumer* diags)
      : builder_(builder), index_pass_(index_pass), diags_(diags) {}

  bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
    ci.getDiagnostics().setClient(diags_, /*OwnsClient=*/false);
    ci.getPreprocessor().addPPCallbacks(
        std::make_unique<IncludeRecorder>(ci.getSourceManager(), &log_));
    if (!index_pass_) {
      recorder_ = std::make_unique<TokenRecorder>(ci.getPreprocessor());
    }
    return true;
  }

  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    class Consumer : public clang::ASTConsumer {
     public:
      explicit Consumer(PipelineAction& owner) : owner_(owner) {}
      void HandleTranslationUnit(clang::ASTContext& ctx) override {
        // Consume before any rewriting: the collector is complete only
        // once the whole top-level loop has run.
        const clang::syntax::TokenBuffer* tokens =
            owner_.recorder_ == nullptr ? nullptr
                                        : &owner_.recorder_->Consume();
        if (owner_.index_pass_) {
          owner_.builder_.IndexTu(ctx, owner_.log_);
        } else {
          owner_.builder_.RewriteTreeTu(ctx, std::move(owner_.log_), tokens);
        }
      }

     private:
      PipelineAction& owner_;
    };
    return std::make_unique<Consumer>(*this);
  }

 private:
  ProgramBuilder& builder_;
  bool index_pass_;
  CollectingDiagConsumer* diags_;
  std::vector<IncludeDirective> log_;
  std::unique_ptr<TokenRecorder> recorder_;
};

}  // namespace

TreePipelineRun RunTreePipeline(
    const std::vector<VirtualTu>& tus, const std::string& top,
    const std::string& out_root, const std::vector<std::string>& args,
    const clang::tooling::FileContentMappings& virtual_files) {
  std::vector<std::string> main_files;
  main_files.reserve(tus.size());
  for (const VirtualTu& tu : tus) main_files.push_back(CanonicalPath(tu.path));
  CollectingDiagConsumer diags;
  ProgramBuilder builder(top, SynthTarget::kXilinxHls,
                         TreeConfig{out_root, std::move(main_files)});

  // The stub declarations ride in front of each TU's own text; virtual
  // header files stand alone so several TUs can sight one shared header.
  const auto run_pass = [&](bool index_pass) {
    for (const VirtualTu& tu : tus) {
      clang::tooling::runToolOnCodeWithArgs(
          std::make_unique<PipelineAction>(builder, index_pass, &diags),
          std::string(kTapaStubDecls) + "\n" + tu.code,
          tu.own_args.empty() ? args : tu.own_args, tu.path, "tree_pipeline",
          std::make_shared<clang::PCHContainerOperations>(), virtual_files);
    }
  };
  llvm::sys::fs::remove_directories(out_root);
  llvm::sys::fs::create_directories(out_root);
  // tapacc aborts at the first failed stage: an error diagnostic fails the
  // pass it was emitted in, a failed merge skips the rewrite entirely.
  run_pass(/*index_pass=*/true);
  bool ok = diags.errors.empty() && builder.MergeAndDiscover();
  if (ok) {
    run_pass(/*index_pass=*/false);
    std::string error;
    ok = diags.errors.empty() && builder.WriteTree(&error);
  }

  TreePipelineRun run{std::move(builder)};
  run.diags = diags.errors;
  run.json = run.builder.EmitJson().dump();
  run.ok = ok;
  if (ok) run.files = ReadTree(out_root);
  return run;
}

}  // namespace tapa::cc::test_support
