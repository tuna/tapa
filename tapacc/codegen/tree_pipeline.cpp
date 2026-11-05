#include "tree_pipeline.h"

#include <memory>

#include "clang/Tooling/Tooling.h"
#include "llvm/Support/FileSystem.h"

#include "builder_action.h"
#include "diag_capture.h"
#include "frontend/tapa_stub_decls.h"
#include "test_util.h"
#include "tree_writer.h"

namespace tapa::cc::test_support {

TreePipelineRun RunTreePipeline(
    const std::vector<VirtualTu>& tus, const std::string& top,
    const std::string& out_root, const std::vector<std::string>& args,
    const clang::tooling::FileContentMappings& virtual_files,
    std::map<std::string, SynthTarget> task_targets) {
  std::vector<std::string> main_files;
  main_files.reserve(tus.size());
  for (const VirtualTu& tu : tus) main_files.push_back(CanonicalPath(tu.path));
  CollectingDiagConsumer diags;
  ProgramBuilder builder(top, SynthTarget::kXilinxHls,
                         TreeConfig{out_root, std::move(main_files)});
  builder.SetTaskTargets(std::move(task_targets));

  // The stub declarations ride in front of each TU's own text; virtual
  // header files stand alone so several TUs can sight one shared header.
  const auto run_pass = [&](bool index_pass) {
    for (const VirtualTu& tu : tus) {
      clang::tooling::runToolOnCodeWithArgs(
          std::make_unique<BuilderAction>(builder, index_pass,
                                          /*vendor_scan=*/false, &diags),
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
    ok = diags.errors.empty() && builder.CheckAieEot() &&
         builder.WriteTree(&error);
  }

  TreePipelineRun run{std::move(builder)};
  run.diags = diags.errors;
  run.json = run.builder.EmitJson().dump();
  run.ok = ok;
  if (ok) run.files = ReadTree(out_root);
  return run;
}

}  // namespace tapa::cc::test_support
