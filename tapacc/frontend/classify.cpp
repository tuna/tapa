#include "classify.h"

#include "clang/AST/DeclTemplate.h"
#include "llvm/ADT/StringSwitch.h"

#include "frontend/type_args.h"

namespace tapa::cc {

namespace {

TapaKind ClassifyByQualifiedName(llvm::StringRef name) {
  // Keys are the qualified record names Clang prints for the TAPA class
  // templates (without template arguments), matched exactly.
  return llvm::StringSwitch<TapaKind>(name)
      .Case("tapa::istream", TapaKind::kIStream)
      .Case("tapa::ostream", TapaKind::kOStream)
      .Case("tapa::istreams", TapaKind::kIStreams)
      .Case("tapa::ostreams", TapaKind::kOStreams)
      .Case("tapa::iframes", TapaKind::kIFrames)
      .Case("tapa::oframes", TapaKind::kOFrames)
      .Case("tapa::stream", TapaKind::kStream)
      .Case("tapa::streams", TapaKind::kStreams)
      .Case("tapa::mmap", TapaKind::kMmap)
      .Case("tapa::mmaps", TapaKind::kMmaps)
      .Case("tapa::async_mmap", TapaKind::kAsyncMmap)
      .Case("tapa::hmap", TapaKind::kHmap)
      .Case("tapa::task", TapaKind::kTask)
      .Case("tapa::seq", TapaKind::kSeq)
      .Case("tapa::executable", TapaKind::kExecutable)
      .Default(TapaKind::kNotTapa);
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
    case TapaKind::kIFrames:
      return "iframes";
    case TapaKind::kOFrames:
      return "oframes";
    case TapaKind::kMmap:
    case TapaKind::kMmaps:
    case TapaKind::kHmap:
      return "mmap";
    case TapaKind::kAsyncMmap:
      return "async_mmap";
    default:
      return "scalar";
  }
}

}  // namespace tapa::cc
