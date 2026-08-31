#ifndef TAPA_CODEGEN_TREE_PIPELINE_H_
#define TAPA_CODEGEN_TREE_PIPELINE_H_

#include <map>
#include <string>
#include <utility>
#include <vector>

#include "clang/Tooling/Tooling.h"

#include "frontend/program_builder.h"

namespace tapa::cc::test_support {

// One virtual translation unit: a path plus its user code. The TAPA stub
// declarations (frontend/tapa_stub_decls.h) are prepended automatically,
// standing in for `#include <tapa.h>`.
struct VirtualTu {
  std::string path;
  std::string code;
  // TU-local compile flags (a TU-local define is how a shared header's
  // macro can expand differently per TU); empty rides the shared `args`.
  std::vector<std::string> own_args;
  VirtualTu(std::string p, std::string c, std::vector<std::string> a = {})
      : path(std::move(p)), code(std::move(c)), own_args(std::move(a)) {}
};

// The outcome of one pipeline run.
struct TreePipelineRun {
  // The rendered mirrored tree, tree-relative path -> bytes. Empty when the
  // run failed before materialization.
  std::map<std::string, std::string> files;
  // Every error diagnostic of both passes, formatted text in emission
  // order.
  std::vector<std::string> diags;
  // The single graph JSON, as tapa-ir consumes it.
  std::string json;
  // The builder after the run; graph queries (FindTask, errors) stay valid.
  ProgramBuilder builder;
  // True when the merge, every rewrite pass, and the tree materialization
  // all ran clean.
  bool ok = false;

  explicit TreePipelineRun(ProgramBuilder built) : builder(std::move(built)) {}
};

// Runs exactly what `tapacc` runs over the virtual TUs: an index action per
// TU (include log recorded, no tokens), the merge, then a rewrite action per
// TU with the token stream recorded, then the tree materialization under
// `out_root` (wiped and created). `virtual_files` maps extra virtual paths
// (shared headers) to content, taken verbatim with no stubs.
TreePipelineRun RunTreePipeline(
    const std::vector<VirtualTu>& tus, const std::string& top,
    const std::string& out_root, const std::vector<std::string>& args,
    const clang::tooling::FileContentMappings& virtual_files = {});

}  // namespace tapa::cc::test_support

#endif  // TAPA_CODEGEN_TREE_PIPELINE_H_
