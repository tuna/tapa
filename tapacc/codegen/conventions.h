#ifndef TAPA_CODEGEN_CONVENTIONS_H_
#define TAPA_CODEGEN_CONVENTIONS_H_

#include <map>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

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

// The preprocessor define selecting one task definition in the rewritten
// tree. Graph-key bytes outside the identifier alphabet become underscores;
// ProgramBuilder rejects distinct keys that sanitize to the same define.
inline std::string TaskGuardDefine(std::string_view task_key) {
  std::string define = "TAPA_TASK_DEF_";
  for (const unsigned char c : task_key) {
    const bool safe = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                      (c >= '0' && c <= '9') || c == '_';
    define.push_back(safe ? static_cast<char>(c) : '_');
  }
  return define;
}

// The hard diagnostic for two distinct graph keys selecting one guard after
// sanitization; duplicate sightings of the same key are one fact.
inline std::optional<std::string> TaskGuardCollision(
    const std::vector<std::string>& task_keys) {
  std::map<std::string, std::string> owners;
  for (const std::string& key : task_keys) {
    const std::string guard = TaskGuardDefine(key);
    const auto [owner, inserted] = owners.emplace(guard, key);
    if (!inserted && owner->second != key) {
      return "task keys '" + owner->second + "' and '" + key +
             "' both sanitize to rewritten-tree guard '" + guard + "'";
    }
  }
  return std::nullopt;
}

// Prefix for mangled task names, so an emitted symbol never starts with '_'
// (which Vitis rejects).
inline constexpr std::string_view kMangledPrefix = "tapa_mangled";

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_CONVENTIONS_H_
