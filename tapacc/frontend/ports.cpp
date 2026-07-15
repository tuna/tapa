#include "ports.h"

#include <optional>
#include <string>
#include <utility>

#include "classify.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

// name[i] — the per-channel spelling for an expanded array interface.
std::string ArrayElemName(const std::string& name, uint32_t i) {
  return name + "[" + std::to_string(i) + "]";
}

// The 0th template argument printed as a C++ type (element type), e.g.
// "const float" for `tapa::mmap<const float>`. Empty if absent.
std::string ElementTypeName(const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    return TemplateArgName(*arg);
  }
  return "";
}

// Bit width of the element (0th template arg) type, or 0 if absent.
uint32_t ElementWidth(const clang::ASTContext& ctx,
                      const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    if (arg->getKind() == clang::TemplateArgument::Type) {
      return BitWidth(ctx, arg->getAsType());
    }
  }
  return 0;
}

// An integral template argument at `idx` as a channel count, or nullopt.
std::optional<uint32_t> IntArg(const clang::ParmVarDecl* param, unsigned idx) {
  if (const auto n = IntTemplateArg(param->getType(), idx)) {
    return static_cast<uint32_t>(*n);
  }
  return std::nullopt;
}

}  // namespace

std::vector<Port> BuildPorts(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* task) {
  std::vector<Port> ports;
  for (const clang::ParmVarDecl* param : task->parameters()) {
    const TapaKind kind = ClassifyTapaType(param);
    const std::string name = param->getNameAsString();
    const std::string elem = ElementTypeName(param);
    const uint32_t elem_width = ElementWidth(ctx, param);

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
          ports.push_back(Port{ArrayElemName(name, i), TapaKind::kMmap,
                               elem + "*", elem_width, std::nullopt,
                               std::nullopt});
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
