#ifndef TAPA_FRONTEND_TYPE_ARGS_H_
#define TAPA_FRONTEND_TYPE_ARGS_H_

#include <cstdint>
#include <optional>
#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/TemplateBase.h"
#include "clang/AST/Type.h"

namespace tapa::cc {

// Strip lvalue-references and template-parameter substitutions to a fixpoint,
// so that `tapa::mmap<T>&`, `const tapa::mmap<T>&`, and a substituted `T` all
// reduce to the underlying type. This is the one definition of "normalize a
// QualType before inspecting it"; classification and template-argument lookup
// must agree on it or they disagree about what a port is.
clang::QualType PeelType(clang::QualType type);

// The `idx`-th template argument of a (reference/substitution-peeled) type, or
// nullptr if there is none. Never asserts on absence (the old code did).
const clang::TemplateArgument* GetTemplateArg(clang::QualType type,
                                              unsigned idx);

// Print a template argument as its C++ spelling (e.g. "const float"), matching
// the element-type strings the old rewriter emitted for port metadata.
std::string TemplateArgName(const clang::TemplateArgument& arg);

// A template argument as a compile-time integer, or nullopt if it is not
// integral. Uses APSInt::getSExtValue (not raw APInt words) — §8.2.
std::optional<int64_t> TemplateArgAsInt(const clang::TemplateArgument& arg);

// The `idx`-th template argument as a compile-time integer, resolved via the
// canonical class-template specialization so as-written non-type arguments are
// evaluated (channel counts, FIFO depths, hmap sizes). nullopt if absent or
// non-integral. Prefer this over GetTemplateArg + TemplateArgAsInt for ints.
std::optional<int64_t> IntTemplateArg(clang::QualType type, unsigned idx);

// Bit width of a type per the ASTContext (e.g. 32 for float).
uint32_t BitWidth(const clang::ASTContext& ctx, clang::QualType type);

// The element (0th template arg) type of a stream/mmap port, printed as its
// C++ spelling (e.g. "const float" for `tapa::mmap<const float>`). Empty if
// absent. Shared by the frontend port model and the codegen backends.
std::string ElementTypeName(const clang::ParmVarDecl* param);

// Bit width of the element (0th template arg) type of a port, or 0 if absent.
uint32_t ElementWidth(const clang::ParmVarDecl* param);

// Channel count (template arg at index 1) of an array-interface port, or 0.
int64_t ArraySize(const clang::ParmVarDecl* param);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_TYPE_ARGS_H_
