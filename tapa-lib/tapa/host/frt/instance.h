#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include "tapa/host/frt/c_api.h"
#include "tapa/host/frt/types.h"

namespace tapa {
namespace internal {
namespace frt {

// A stream argument the host binds, as the kernel metadata declares it.
struct HostStream {
  int index;
  std::string name;
  bool kernel_reads;  // the host writes it
};

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
  // Passes a buffer of `bytes` bytes at `ptr`; `access` states what the
  // kernel does with it, which is what decides the transfers.
  void SetBufferArg(int index, const void* ptr, size_t bytes,
                    BufferAccess access);
  // Passes a shared-memory stream identified by `path`.
  void SetStreamArg(int index, const std::string& path);
  // The stream arguments the host binds, in argument order; none on
  // hardware.
  std::vector<HostStream> HostStreams() const;
  // Whether the instance runs only the design's AI Engine graph, so the
  // host runs every other task in software and closes each stream the
  // instance reads once its software producer returns.
  bool SimulatesAie() const;

  void WriteToDevice();
  void ReadFromDevice();
  void Exec();
  void Finish();
  void Kill();
  bool IsFinished() const;

  int64_t ComputeTimeNanoSeconds() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace frt
}  // namespace internal
}  // namespace tapa
