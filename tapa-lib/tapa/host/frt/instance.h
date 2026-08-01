#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <ostream>
#include <string>
#include <vector>

#include "tapa/host/frt/c_api.h"
#include "tapa/host/frt/types.h"

namespace tapa {
namespace internal {
namespace frt {

struct ArgInfo {
  int index = 0;
  std::string name;
  std::string type;
  RuntimeArgCategory cat = RuntimeArgCategory::Scalar;
};

inline std::ostream& operator<<(std::ostream& os,
                                const RuntimeArgCategory& cat) {
  switch (cat) {
    case RuntimeArgCategory::Scalar:
      return os << "scalar";
    case RuntimeArgCategory::Mmap:
      return os << "mmap";
    case RuntimeArgCategory::Stream:
      return os << "stream";
    case RuntimeArgCategory::Streams:
      return os << "streams";
  }
  return os << "unknown";
}

inline std::ostream& operator<<(std::ostream& os, const ArgInfo& arg) {
  return os << "ArgInfo(index=" << arg.index << ", name=" << arg.name
            << ", type=" << arg.type << ", cat=" << arg.cat << ")";
}

// Thin RAII handle over the Rust `frt_instance_*` C ABI. Argument lowering is
// performed once by tapa-lib's accessors, which call the three `Set*Arg`
// methods directly; there is no additional templated marshalling layer here.
class Instance {
 public:
  explicit Instance(const std::string& bitstream);
  Instance(Instance&&) noexcept;
  Instance& operator=(Instance&&) noexcept;
  ~Instance();

  // Copies `size` bytes from `data` as a scalar argument at `index`.
  void SetScalarArg(int index, const void* data, size_t size);
  // Passes a buffer of `bytes` bytes at `ptr` with access category `tag`.
  void SetBufferArg(int index, const void* ptr, size_t bytes, Tag tag);
  // Passes a shared-memory stream identified by `path`.
  void SetStreamArg(int index, const std::string& path);

  size_t SuspendBuf(int index);
  void WriteToDevice();
  void ReadFromDevice();
  void Exec();
  void Pause();
  void Resume();
  void Finish();
  void Kill();
  bool IsFinished() const;

  std::vector<ArgInfo> GetArgsInfo() const;
  int64_t LoadTimeNanoSeconds() const;
  int64_t ComputeTimeNanoSeconds() const;
  int64_t StoreTimeNanoSeconds() const;
  double LoadTimeSeconds() const;
  double ComputeTimeSeconds() const;
  double StoreTimeSeconds() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace frt
}  // namespace internal
}  // namespace tapa
