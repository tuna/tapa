#include "classify.h"

#include <string_view>
#include <unordered_map>

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
      {"tapa::async_mmaps", TapaKind::kAsyncMmaps},
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
  // Peel lvalue-references and template-parameter substitutions so that
  // `tapa::mmap<T>&`, `const tapa::mmap<T>&`, and a substituted `T` all reduce
  // to the underlying record.
  for (;;) {
    if (const auto* ref = type->getAs<clang::LValueReferenceType>()) {
      type = ref->getPointeeType();
      continue;
    }
    if (const auto* subst = type->getAs<clang::SubstTemplateTypeParmType>()) {
      type = subst->getReplacementType();
      continue;
    }
    break;
  }
  const clang::RecordDecl* record = type->getAsRecordDecl();
  if (record == nullptr) return TapaKind::kNotTapa;
  return ClassifyByQualifiedName(record->getQualifiedNameAsString());
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
      return "mmap";
    case TapaKind::kAsyncMmap:
    case TapaKind::kAsyncMmaps:
      return "async_mmap";
    case TapaKind::kImmap:
      return "immap";
    case TapaKind::kOmmap:
      return "ommap";
    case TapaKind::kHmap:
      return "hmap";
    default:
      return "scalar";
  }
}

}  // namespace tapa::cc
