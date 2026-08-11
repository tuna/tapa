#include "ports.h"

#include <optional>
#include <string>
#include <utility>

#include "clang/Basic/Diagnostic.h"

#include "classify.h"
#include "codegen/conventions.h"
#include "diag.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

// An integral template argument at `idx` as a channel count, or nullopt.
std::optional<uint32_t> IntArg(const clang::ParmVarDecl* param, unsigned idx) {
  if (const auto n = IntTemplateArg(param->getType(), idx)) {
    return static_cast<uint32_t>(*n);
  }
  return std::nullopt;
}

// Exact `tapa::` spelling of a port kind, for diagnostics.
const char* KindSpelling(TapaKind k) {
  switch (k) {
    case TapaKind::kIStream:
      return "tapa::istream";
    case TapaKind::kOStream:
      return "tapa::ostream";
    case TapaKind::kIStreams:
      return "tapa::istreams";
    case TapaKind::kOStreams:
      return "tapa::ostreams";
    case TapaKind::kAsyncMmap:
      return "tapa::async_mmap";
    case TapaKind::kMmap:
      return "tapa::mmap";
    case TapaKind::kMmaps:
      return "tapa::mmaps";
    case TapaKind::kImmap:
      return "tapa::immap";
    case TapaKind::kOmmap:
      return "tapa::ommap";
    case TapaKind::kHmap:
      return "tapa::hmap";
    default:
      return "tapa::?";
  }
}

// Parameter-shape contract: stream channels and `async_mmap` have connection
// identity and must be passed by reference; mmap-family types are
// pointer-like handles and must be passed by value.
void CheckParamShape(const clang::ASTContext& ctx,
                     const clang::ParmVarDecl* param, TapaKind kind) {
  switch (kind) {
    case TapaKind::kIStream:
    case TapaKind::kOStream:
    case TapaKind::kIStreams:
    case TapaKind::kOStreams:
    case TapaKind::kAsyncMmap:
      if (!param->getType()->isLValueReferenceType()) {
        auto builder = ReportCustomDiag(
            ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
            "%0 parameter '%1' must be passed by reference");
        builder.AddString(KindSpelling(ClassifyTapaType(param)));
        builder.AddString(param->getNameAsString());
      }
      break;
    // `tapa::stream`/`tapa::streams` declare a channel; they carry no
    // direction and are only meaningful as a local variable of an upper task.
    // As a parameter they used to fall through to the scalar catch-all, which
    // bound the channel object to the s_axilite control bundle and left Vitis
    // HLS to fail on it much later ("Cannot apply disaggregate pragma ...").
    case TapaKind::kStream:
    case TapaKind::kStreams: {
      const bool array = kind == TapaKind::kStreams;
      auto builder = ReportCustomDiag(
          ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
          "%0 parameter '%1' declares a channel rather than a port; a task "
          "reads a channel through tapa::%2 and writes one through tapa::%3. "
          "%0 belongs in the body of an upper task, where it connects two "
          "invocations");
      builder.AddString(array ? "tapa::streams" : "tapa::stream");
      builder.AddString(param->getNameAsString());
      builder.AddString(array ? "istreams" : "istream");
      builder.AddString(array ? "ostreams" : "ostream");
      break;
    }
    case TapaKind::kMmap:
    case TapaKind::kMmaps:
    case TapaKind::kImmap:
    case TapaKind::kOmmap:
    case TapaKind::kHmap:
      if (param->getType()->isReferenceType()) {
        auto builder = ReportCustomDiag(
            ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
            "%0 parameter '%1' must be passed by value "
            "(not by reference)");
        builder.AddString(KindSpelling(ClassifyTapaType(param)));
        builder.AddString(param->getNameAsString());
      }
      break;
    default:
      break;
  }
}

}  // namespace

std::vector<Port> BuildPorts(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* task) {
  std::vector<Port> ports;
  for (const clang::ParmVarDecl* param : task->parameters()) {
    const TapaKind kind = ClassifyTapaType(param);
    CheckParamShape(ctx, param, kind);
    const std::string name = param->getNameAsString();
    const std::string elem = ElementTypeName(param);
    const uint32_t elem_width = ElementWidth(param);

    switch (kind) {
      case TapaKind::kMmap:
      case TapaKind::kImmap:
      case TapaKind::kOmmap:
      case TapaKind::kAsyncMmap:
        ports.push_back(Port{name, kind, elem + "*", elem_width, std::nullopt,
                             std::nullopt});
        break;

      case TapaKind::kMmaps: {
        // Expand to one `mmap` port per channel (name[i]).
        const uint32_t n = IntArg(param, 1).value_or(0);
        for (uint32_t i = 0; i < n; ++i) {
          ports.push_back(Port{ArrayNameAt(name, static_cast<int>(i)),
                               TapaKind::kMmap, elem + "*", elem_width,
                               std::nullopt, std::nullopt});
        }
        break;
      }

      case TapaKind::kHmap:
        ports.push_back(Port{name, kind, elem + "*", elem_width,
                             IntArg(param, 1), IntArg(param, 2)});
        break;

      case TapaKind::kIStream:
      case TapaKind::kOStream:
        ports.push_back(
            Port{name, kind, elem, elem_width, std::nullopt, std::nullopt});
        break;

      case TapaKind::kIStreams:
      case TapaKind::kOStreams:
        ports.push_back(
            Port{name, kind, elem, elem_width, IntArg(param, 1), std::nullopt});
        break;

      default:
        // Scalar: the parameter's own type and width.
        ports.push_back(
            Port{name, TapaKind::kNotTapa, param->getType().getAsString(),
                 BitWidth(ctx, param->getType()), std::nullopt, std::nullopt});
        break;
    }
  }
  return ports;
}

}  // namespace tapa::cc
