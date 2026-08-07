#include "classify.h"

#include <string_view>
#include <unordered_map>

#include "clang/AST/DeclTemplate.h"

#include "frontend/type_args.h"

namespace tapa::cc {

namespace {

TapaKind ClassifyByQualifiedName(std::string_view name) {
  // Keys are the qualified record names Clang prints for the TAPA class
  // templates (without template arguments), matched exactly.
  static const std::unordered_map<std::string_view, TapaKind> kTable = {
      {"tapa::istream", TapaKind::kIStream},
      {"tapa::ostream", TapaKind::kOStream},
      {"tapa::istreams", TapaKind::kIStreams},
      {"tapa::ostreams", TapaKind::kOStreams},
      {"tapa::stream", TapaKind::kStream},
      {"tapa::streams", TapaKind::kStreams},
      {"tapa::mmap", TapaKind::kMmap},
      {"tapa::mmaps", TapaKind::kMmaps},
      {"tapa::async_mmap", TapaKind::kAsyncMmap},
      {"tapa::immap", TapaKind::kImmap},
      {"tapa::ommap", TapaKind::kOmmap},
      {"tapa::hmap", TapaKind::kHmap},
      {"tapa::task", TapaKind::kTask},
      {"tapa::seq", TapaKind::kSeq},
      {"tapa::executable", TapaKind::kExecutable},
  };
  const auto it = kTable.find(name);
  return it == kTable.end() ? TapaKind::kNotTapa : it->second;
}

}  // namespace

TapaKind ClassifyTapaType(clang::QualType type) {
  type = PeelType(type);
  if (const clang::RecordDecl* record = type->getAsRecordDecl()) {
    return ClassifyByQualifiedName(record->getQualifiedNameAsString());
  }
  // A dependent specialization (e.g. `tapa::mmap<mmap_type>` inside a template)
  // has no record decl; classify by the template name instead. A bare template
  // parameter type (e.g. `tapa_mmap_type`) has neither and stays kNotTapa.
  if (const auto* spec = type->getAs<clang::TemplateSpecializationType>()) {
    if (const clang::TemplateDecl* decl =
            spec->getTemplateName().getAsTemplateDecl()) {
      return ClassifyByQualifiedName(decl->getQualifiedNameAsString());
    }
  }
  return TapaKind::kNotTapa;
}

const char* TapaKindCat(TapaKind k) {
  switch (k) {
    case TapaKind::kIStream:
      return "istream";
    case TapaKind::kOStream:
      return "ostream";
    case TapaKind::kIStreams:
      return "istreams";
    case TapaKind::kOStreams:
      return "ostreams";
    case TapaKind::kMmap:
    case TapaKind::kMmaps:
    case TapaKind::kHmap:
      return "mmap";
    case TapaKind::kAsyncMmap:
      return "async_mmap";
    case TapaKind::kImmap:
      return "immap";
    case TapaKind::kOmmap:
      return "ommap";
    default:
      return "scalar";
  }
}

}  // namespace tapa::cc
