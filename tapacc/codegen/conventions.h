#ifndef TAPA_CODEGEN_CONVENTIONS_H_
#define TAPA_CODEGEN_CONVENTIONS_H_

#include <string>
#include <string_view>

namespace tapa::cc {

// The tapacc <-> tapa-rtl / tapa-ir textual ABI: the exact spellings the
// downstream tools expect in generated code and metadata. Changing any of these
// is a cross-tool break, so they live in one documented place.

// The scalar base-address parameter an mmap lowers to: "<base>_offset".
inline std::string OffsetName(std::string_view base) {
  return std::string(base) + "_offset";
}

// The i-th channel of an `mmaps` port as a flat offset parameter:
// "<base>_<i>_offset".
inline std::string ArrayElemOffset(std::string_view base, int i) {
  return std::string(base) + "_" + std::to_string(i) + "_offset";
}

// The i-th element of an array interface as a C++ subscript: "<base>[<i>]".
inline std::string ArrayNameAt(std::string_view base, int i) {
  return std::string(base) + "[" + std::to_string(i) + "]";
}

// The internal FIFO member of a stream: "<name>._".
inline std::string FifoVar(std::string_view name) {
  return std::string(name) + "._";
}

// The peek FIFO member of an istream: "<name>._peek".
inline std::string PeekVar(std::string_view name) {
  return std::string(name) + "._peek";
}

// Prefix for mangled task names, so an emitted symbol never starts with '_'
// (which Vitis rejects).
inline constexpr std::string_view kMangledPrefix = "tapa_mangled";

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_CONVENTIONS_H_
