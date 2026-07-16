#ifndef TAPA_FRONTEND_CLASSIFY_H_
#define TAPA_FRONTEND_CLASSIFY_H_

#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/Type.h"

namespace tapa::cc {

// Structural classification of a task-parameter (or argument) type against the
// fixed set of TAPA types. Replaces the old regex matching on stringified
// qualified names (`IsTapaType(x, "((async_)?mmaps|hmap)")`): a type like
// `tapa::mmap_wrapper` can never be misread, and there is no regex surface.
enum class TapaKind {
  kNotTapa,  // any non-TAPA type: treated as a scalar port

  // stream interfaces (task ports)
  kIStream,
  kOStream,
  kIStreams,
  kOStreams,

  // stream instances (FIFOs declared inside an upper task; not ports)
  kStream,
  kStreams,

  // memory-mapped interfaces
  kMmap,
  kMmaps,
  kAsyncMmap,
  kImmap,
  kOmmap,
  kHmap,

  // special markers
  kTask,        // tapa::task (the invoke builder)
  kSeq,         // tapa::seq
  kExecutable,  // tapa::executable
};

// Classify a type, peeling lvalue-references and template-parameter
// substitutions so `tapa::mmap<T>&` and a substituted `T` both reduce to the
// underlying record.
TapaKind ClassifyTapaType(clang::QualType type);

inline TapaKind ClassifyTapaType(const clang::ParmVarDecl* param) {
  return ClassifyTapaType(param->getType());
}
inline TapaKind ClassifyTapaType(const clang::Expr* expr) {
  return ClassifyTapaType(expr->getType());
}

// --- category predicates (enum checks, no strings) ---

// istream / ostream / istreams / ostreams (any stream *port*).
constexpr bool IsStreamInterface(TapaKind k) {
  return k == TapaKind::kIStream || k == TapaKind::kOStream ||
         k == TapaKind::kIStreams || k == TapaKind::kOStreams;
}
// istreams / ostreams (an array of streams).
constexpr bool IsStreamArray(TapaKind k) {
  return k == TapaKind::kIStreams || k == TapaKind::kOStreams;
}
constexpr bool IsInputStream(TapaKind k) {
  return k == TapaKind::kIStream || k == TapaKind::kIStreams;
}
constexpr bool IsOutputStream(TapaKind k) {
  return k == TapaKind::kOStream || k == TapaKind::kOStreams;
}
// stream / streams (a FIFO *instance*, declared inside an upper task).
constexpr bool IsStreamInstance(TapaKind k) {
  return k == TapaKind::kStream || k == TapaKind::kStreams;
}
// mmap / mmaps / async_mmap / immap / ommap / hmap.
constexpr bool IsMmapInterface(TapaKind k) {
  return k == TapaKind::kMmap || k == TapaKind::kMmaps ||
         k == TapaKind::kAsyncMmap || k == TapaKind::kImmap ||
         k == TapaKind::kOmmap || k == TapaKind::kHmap;
}
constexpr bool IsAsyncMmap(TapaKind k) { return k == TapaKind::kAsyncMmap; }
// An array-shaped interface (expands to N wires): *streams, mmaps, hmap.
constexpr bool IsArrayInterface(TapaKind k) {
  return IsStreamArray(k) || k == TapaKind::kMmaps || k == TapaKind::kHmap;
}

// The canonical serialized category ("cat") for a port/arg, e.g. "istream",
// "mmap", "scalar". Centralized with the enum; the values are the stable
// `ArgCategory` strings the graph schema keeps regardless of the tapa-ir
// rename.
const char* TapaKindCat(TapaKind k);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_CLASSIFY_H_
