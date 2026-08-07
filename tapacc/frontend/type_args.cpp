#include "type_args.h"

#include "clang/AST/Decl.h"
#include "clang/AST/DeclTemplate.h"
#include "clang/Basic/LangOptions.h"
#include "llvm/Support/raw_ostream.h"

namespace tapa::cc {

clang::QualType PeelType(clang::QualType type) {
  for (bool changed = true; changed;) {
    changed = false;
    if (const auto* ref = type->getAs<clang::LValueReferenceType>()) {
      type = ref->getPointeeType();
      changed = true;
    }
    if (const auto* subst = type->getAs<clang::SubstTemplateTypeParmType>()) {
      type = subst->getReplacementType();
      changed = true;
    }
  }
  return type;
}

const clang::TemplateArgument* GetTemplateArg(clang::QualType type,
                                              unsigned idx) {
  type = PeelType(type);

  // Prefer the as-written specialization type so type arguments keep their
  // source spelling (e.g. a typedef stays a typedef).
  if (const auto* spec = type->getAs<clang::TemplateSpecializationType>()) {
    const auto args = spec->template_arguments();
    if (idx < args.size()) return &args[idx];
  }
  if (const auto* record = type->getAs<clang::RecordType>()) {
    if (const auto* decl =
            llvm::dyn_cast<clang::ClassTemplateSpecializationDecl>(
                record->getDecl())) {
      const auto& args = decl->getTemplateArgs();
      if (idx < args.size()) return &args[idx];
    }
  }
  return nullptr;
}

std::optional<int64_t> IntTemplateArg(clang::QualType type, unsigned idx) {
  // Resolve via the canonical class-template specialization: unlike the
  // as-written `TemplateSpecializationType`, its arguments have non-type
  // parameters already evaluated to `Integral` (the as-written form stores
  // them as unevaluated `Expression`s).
  type = PeelType(type);
  const auto* record = type->getAs<clang::RecordType>();
  if (record == nullptr) return std::nullopt;
  const auto* spec =
      llvm::dyn_cast<clang::ClassTemplateSpecializationDecl>(record->getDecl());
  if (spec == nullptr) return std::nullopt;
  const auto& args = spec->getTemplateArgs();
  if (idx >= args.size()) return std::nullopt;
  return TemplateArgAsInt(args[idx]);
}

std::string TemplateArgName(const clang::TemplateArgument& arg) {
  std::string name;
  llvm::raw_string_ostream os(name);
  clang::LangOptions options;
  options.CPlusPlus = true;
  options.Bool = true;
  const clang::PrintingPolicy policy(options);
  arg.print(policy, os, /*IncludeType=*/false);
  return name;
}

std::optional<int64_t> TemplateArgAsInt(const clang::TemplateArgument& arg) {
  if (arg.getKind() == clang::TemplateArgument::Integral) {
    return arg.getAsIntegral().getSExtValue();
  }
  return std::nullopt;
}

uint32_t BitWidth(const clang::ASTContext& ctx, clang::QualType type) {
  return static_cast<uint32_t>(ctx.getTypeSize(type));
}

std::string ElementTypeName(const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    return TemplateArgName(*arg);
  }
  return "";
}

uint32_t ElementWidth(const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    if (arg->getKind() == clang::TemplateArgument::Type) {
      return BitWidth(param->getASTContext(), arg->getAsType());
    }
  }
  return 0;
}

int64_t ArraySize(const clang::ParmVarDecl* param) {
  return IntTemplateArg(param->getType(), 1).value_or(0);
}

}  // namespace tapa::cc
