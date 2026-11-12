#include "ports.h"

#include <cstdint>
#include <limits>
#include <optional>
#include <string>

#include "clang/AST/Attr.h"
#include "clang/Basic/Diagnostic.h"

#include "classify.h"
#include "codegen/conventions.h"
#include "diag.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

// An integral template argument at `idx`, which the task graph records in
// 32 bits, or nullopt when the argument is absent. One that does not fit is
// an error rather than a wrapped value.
std::optional<uint32_t> IntArg(const clang::ASTContext& ctx,
                               const clang::ParmVarDecl* param, unsigned idx,
                               const char* what) {
  const auto n = IntTemplateArg(param->getType(), idx);
  if (!n) return std::nullopt;
  if (*n < 0 || *n > std::numeric_limits<uint32_t>::max()) {
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
                     "parameter '%0' has %1 %2; the task graph records it in "
                     "32 bits")
        << param->getNameAsString() << what << std::to_string(*n);
    return std::nullopt;
  }
  return static_cast<uint32_t>(*n);
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
    case TapaKind::kIFrames:
      return "tapa::iframes";
    case TapaKind::kOFrames:
      return "tapa::oframes";
    case TapaKind::kAsyncMmap:
      return "tapa::async_mmap";
    case TapaKind::kMmap:
      return "tapa::mmap";
    case TapaKind::kMmaps:
      return "tapa::mmaps";
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
    case TapaKind::kIFrames:
    case TapaKind::kOFrames:
    case TapaKind::kAsyncMmap:
      if (!param->getType()->isLValueReferenceType()) {
        auto builder = ReportCustomDiag(
            ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
            "%0 parameter '%1' must be passed by reference");
        builder.AddString(KindSpelling(kind));
        builder.AddString(param->getNameAsString());
      }
      break;
    // `tapa::stream`/`tapa::streams` declare a channel; they carry no
    // direction and are only meaningful as a local variable of an upper task.
    // As a parameter a channel would bind to the s_axilite control bundle as
    // a scalar, and Vitis HLS would fail on it only much later ("Cannot apply
    // disaggregate pragma ...").
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
    case TapaKind::kHmap:
      if (param->getType()->isReferenceType()) {
        auto builder = ReportCustomDiag(
            ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
            "%0 parameter '%1' must be passed by value "
            "(not by reference)");
        builder.AddString(KindSpelling(kind));
        builder.AddString(param->getNameAsString());
      }
      break;
    default:
      if (param->getType().getNonReferenceType()->isPointerType()) {
        auto builder = ReportCustomDiag(
            ctx, clang::DiagnosticsEngine::Error, param->getLocation(),
            "raw pointer task parameter '%0' is not a TAPA memory port; "
            "use tapa::mmap<T> by value for off-chip memory");
        builder.AddString(param->getNameAsString());
      }
      break;
  }
}

}  // namespace

std::vector<Port> BuildPorts(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* task) {
  // Ports as the parameters spell them; the AI Engine knobs are read later,
  // once the task's target is known.
  std::vector<Port> ports;
  for (const clang::ParmVarDecl* param : task->parameters()) {
    const TapaKind kind = ClassifyTapaType(param);
    CheckParamShape(ctx, param, kind);
    const std::string name = param->getNameAsString();
    const std::string elem = ElementTypeName(param);
    const uint32_t elem_width = IsStreamInterface(kind)
                                    ? StreamElementWidth(param)
                                    : ElementWidth(param);

    switch (kind) {
      case TapaKind::kMmap:
      case TapaKind::kAsyncMmap:
        ports.push_back(Port{name, kind, elem + "*", elem_width});
        break;

      case TapaKind::kMmaps: {
        // Expand to one `mmap` port per channel (name[i]).
        const uint32_t n = IntArg(ctx, param, 1, "channel count").value_or(0);
        for (uint32_t i = 0; i < n; ++i) {
          ports.push_back(Port{ArrayNameAt(name, static_cast<int>(i)),
                               TapaKind::kMmap, elem + "*", elem_width});
        }
        break;
      }

      case TapaKind::kHmap:
        ports.push_back(Port{name, kind, elem + "*", elem_width,
                             IntArg(ctx, param, 1, "chan_count"),
                             IntArg(ctx, param, 2, "chan_size"), std::nullopt,
                             std::nullopt, std::nullopt, std::nullopt});
        break;

      case TapaKind::kIStream:
      case TapaKind::kOStream:
        ports.push_back(Port{name, kind, elem, elem_width});
        break;

      case TapaKind::kIStreams:
      case TapaKind::kOStreams:
        ports.push_back(Port{name, kind, elem, elem_width,
                             IntArg(ctx, param, 1, "channel count"),
                             std::nullopt, std::nullopt, std::nullopt,
                             std::nullopt, std::nullopt});
        break;

      // The frame size is the type's; ReadFrameAttrs checks the rest.
      case TapaKind::kIFrames:
      case TapaKind::kOFrames:
        ports.push_back(Port{name, kind, elem, elem_width, std::nullopt,
                             std::nullopt, IntArg(ctx, param, 1, "frame size"),
                             std::nullopt, std::nullopt, std::nullopt});
        break;

      default:
        // Scalar: the parameter's own type and width.
        ports.push_back(Port{name, TapaKind::kNotTapa,
                             param->getType().getAsString(),
                             BitWidth(ctx, param->getType())});
        break;
    }
  }
  return ports;
}

void ReadFrameAttrs(const clang::ASTContext& ctx, TaskModel& task) {
  const bool leaf = task.level == TaskLevel::kLower;
  for (const clang::FunctionDecl* decl : task.def->redecls()) {
    for (const clang::ParmVarDecl* param : decl->parameters()) {
      const TapaKind kind = ClassifyTapaType(param);
      if (IsFramesPort(kind) && decl == task.def && !leaf) {
        ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                         param->getLocation(),
                         "%0 parameter '%1' of upper task '%2': a frame port "
                         "belongs to a leaf task, and its stream to the task "
                         "that invokes it")
            << KindSpelling(kind) << param->getNameAsString() << task.name;
      }
      const auto* frame = param->getAttr<clang::TapaFrameAttr>();
      if (frame == nullptr) continue;
      const bool single_stream =
          kind == TapaKind::kIStream || kind == TapaKind::kOStream;
      if (!leaf || !single_stream || frame->getSize() <= 0) {
        ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                         frame->getLocation(),
                         "[[tapa::frame(N)]] needs a positive N on a "
                         "tapa::istream or tapa::ostream parameter of a leaf "
                         "task ('%0' of task '%1')")
            << param->getNameAsString() << task.name;
      } else if (Port* port = FindPort(task, param->getNameAsString())) {
        port->frame = static_cast<uint32_t>(frame->getSize());
      }
    }
  }
}

}  // namespace tapa::cc
