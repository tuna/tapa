#include "names.h"

#include "llvm/Support/raw_ostream.h"

namespace tapa::cc {

std::string MangledTaskName(clang::MangleContext& mangler,
                            const clang::FunctionDecl* func) {
  std::string name;
  llvm::raw_string_ostream os(name);
  os << "tapa_mangled";
  mangler.mangleName(func, os);
  os.flush();
  return name;
}

std::string ReadableTaskName(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* func) {
  std::string name;
  llvm::raw_string_ostream os(name);
  func->getNameForDiagnostic(os, ctx.getPrintingPolicy(), /*Qualified=*/true);
  os.flush();
  return name;
}

std::string TaskName(clang::MangleContext& mangler,
                     const clang::FunctionDecl* func) {
  if (func->isFunctionTemplateSpecialization()) {
    return MangledTaskName(mangler, func);
  }
  return func->getNameAsString();
}

}  // namespace tapa::cc
