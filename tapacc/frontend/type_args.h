#ifndef TAPA_FRONTEND_TYPE_ARGS_H_
#define TAPA_FRONTEND_TYPE_ARGS_H_

#include <cstdint>
#include <optional>
#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/TemplateBase.h"
#include "clang/AST/Type.h"

namespace tapa::cc {

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

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_TYPE_ARGS_H_
