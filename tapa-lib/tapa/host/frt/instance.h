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

// An upper task instance the instance was built with inlined: the host
// runs its body itself to bind the streams inside it (see `hybrid_run`).
struct InlinedTask {
  std::string parent;  // the inlined instance that invokes it; empty: the top
  int position;        // where it sits among the parent's invokes
  std::string name;    // its name once inlined, which prefixes its streams
};

// An AI Engine task's async_mmap port whose channels the instance leaves to
// the host: the invoke, as an InlinedTask places it, the parameter, and the
// stream of each channel, empty when the body does not use it (see
// `hybrid_run`).
struct MemoryCrossing {
  std::string parent;  // the inlined instance that invokes it; empty: the top
  int position;        // where it sits among the parent's invokes
  int param;           // the parameter's index
  std::string read_requests;
  std::string read_data;
  std::string write_requests;  // each followed by its data
  std::string write_responses;
};

// How an instance that runs only the design's AI Engine graph meets the
// host's software tasks (see `hybrid_run`).
enum class AieSimulation {
  kNone,    // not an AI Engine simulator
  kLive,    // relays every stream while the simulator runs
  kReplay,  // replays a finished software run of the whole design
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
  // The upper task instances the instance was built with inlined, each
  // after its parent; none on hardware.
  std::vector<InlinedTask> InlinedTasks() const;
  // The AI Engine tasks' async_mmap ports the host serves; none on
  // hardware.
  std::vector<MemoryCrossing> MemoryCrossings() const;
  // How the instance meets the host's software tasks if it runs only the
  // design's AI Engine graph. Live, the host runs every task off the
  // graph's side in software and closes each stream the instance reads
  // once its software producer returns; replaying, the host runs the whole
  // design in software and binds a copy of each stream, closed once the
  // software run ends.
  AieSimulation SimulatesAie() const;

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
