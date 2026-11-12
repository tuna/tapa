#include "tapa/host/frt/instance.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <memory>
#include <string>
#include <vector>

#include <glog/logging.h>

#include "tapa/host/frt/c_api.h"

namespace tapa {
namespace internal {
namespace frt {

// Flag ownership lives entirely in C++ (flags.cpp). These forwarders are
// defined there; referencing them keeps flags.cpp linked so its gflags
// `DEFINE_*` registrations survive.
void ForwardFlagsToEnv();
const char* SimulatorFlag();
bool CosimSetupOnly();

namespace {

const char* LastErr() {
  const char* msg = frt_last_error_message();
  return msg == nullptr ? "(unknown libfrt error)" : msg;
}

void CheckFfi(int rc, const char* action) {
  if (rc != 0) {
    LOG(FATAL) << action << " failed: " << LastErr();
  }
}

}  // namespace

struct Instance::Impl {
  void* handle;

  explicit Impl(const std::string& bitstream) {
    ForwardFlagsToEnv();
    handle = frt_instance_open(bitstream.c_str(), SimulatorFlag());
    LOG_IF(FATAL, handle == nullptr)
        << "failed to open '" << bitstream << "': " << LastErr();
  }

  ~Impl() { frt_instance_close(handle); }
};

Instance::Instance(const std::string& bitstream)
    : impl_(std::make_unique<Impl>(bitstream)) {}

Instance::Instance(Instance&&) noexcept = default;
Instance& Instance::operator=(Instance&&) noexcept = default;
Instance::~Instance() = default;

void Instance::SetScalarArg(int index, const void* data, size_t size) {
  CheckFfi(
      frt_instance_set_scalar_bytes(
          impl_->handle, index, reinterpret_cast<const uint8_t*>(data), size),
      "set_scalar");
}

std::vector<HostStream> Instance::HostStreams() const {
  std::vector<HostStream> streams;
  const int count = frt_instance_host_stream_count(impl_->handle);
  LOG_IF(FATAL, count < 0) << "host_stream_count failed: " << LastErr();
  for (int i = 0; i < count; ++i) {
    char name[1024];
    int kernel_reads = 0;
    const int index = frt_instance_host_stream(impl_->handle, i, name,
                                               sizeof(name), &kernel_reads);
    LOG_IF(FATAL, index < 0) << "host_stream failed: " << LastErr();
    HostStream stream{index, name, kernel_reads != 0};
    int count = 0;
    const int ring = frt_instance_host_stream_ring(
        impl_->handle, i, &stream.ring, &count, &stream.frame);
    LOG_IF(FATAL, ring < 0) << "host_stream_ring failed: " << LastErr();
    if (ring == 0) stream.ring = -1;
    stream.ring_count = count != 0;
    streams.push_back(stream);
  }
  return streams;
}

std::vector<InlinedTask> Instance::InlinedTasks() const {
  std::vector<InlinedTask> tasks;
  const int count = frt_instance_inlined_count(impl_->handle);
  LOG_IF(FATAL, count < 0) << "inlined_count failed: " << LastErr();
  for (int i = 0; i < count; ++i) {
    char parent[1024];
    char name[1024];
    const int position = frt_instance_inlined(
        impl_->handle, i, parent, sizeof(parent), name, sizeof(name));
    LOG_IF(FATAL, position < 0) << "inlined failed: " << LastErr();
    tasks.push_back({parent, position, name});
  }
  return tasks;
}

std::vector<MemoryCrossing> Instance::MemoryCrossings() const {
  std::vector<MemoryCrossing> memories;
  const int count = frt_instance_memory_count(impl_->handle);
  LOG_IF(FATAL, count < 0) << "memory_count failed: " << LastErr();
  constexpr size_t kName = 1024;
  for (int i = 0; i < count; ++i) {
    char names[5][kName];
    int param = -1;
    const int position =
        frt_instance_memory(impl_->handle, i, &param, &names[0][0], kName);
    LOG_IF(FATAL, position < 0) << "memory failed: " << LastErr();
    memories.push_back(
        {names[0], position, param, names[1], names[2], names[3], names[4]});
  }
  return memories;
}

AieSimulation Instance::SimulatesAie() const {
  const int mode = frt_instance_aie_simulation(impl_->handle);
  LOG_IF(FATAL, mode < 0 || mode > 2) << "aie_simulation failed: " << LastErr();
  return static_cast<AieSimulation>(mode);
}

void Instance::SetBufferArg(int index, const void* ptr, size_t bytes,
                            BufferAccess access) {
  // The FRT C ABI takes a mutable pointer. Read-only mmaps expose a const
  // element pointer, so const-cast here as the former BufferArg did; the
  // direction is conveyed by `access`, not pointer constness.
  CheckFfi(frt_instance_set_buffer_arg_typed(
               impl_->handle, index,
               const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(ptr)),
               bytes, static_cast<int>(access)),
           "set_buffer");
}

void Instance::SetStreamArg(int index, const std::string& path) {
  CheckFfi(frt_instance_set_stream_arg(impl_->handle, index, path.c_str()),
           "set_stream");
}

void Instance::WriteToDevice() {
  CheckFfi(frt_instance_write_to_device(impl_->handle), "write_to_device");
}

void Instance::ReadFromDevice() {
  CheckFfi(frt_instance_read_from_device(impl_->handle), "read_from_device");
}

void Instance::Exec() { CheckFfi(frt_instance_exec(impl_->handle), "exec"); }

void Instance::Finish() {
  CheckFfi(frt_instance_finish(impl_->handle), "finish");
  if (CosimSetupOnly()) std::exit(0);
}

void Instance::Kill() { CheckFfi(frt_instance_kill(impl_->handle), "kill"); }

bool Instance::IsFinished() const {
  int ret = frt_instance_is_finished(impl_->handle);
  CHECK_GE(ret, 0) << "is_finished failed: " << LastErr();
  return ret != 0;
}

int64_t Instance::ComputeTimeNanoSeconds() const {
  return frt_instance_compute_ns(impl_->handle);
}

}  // namespace frt
}  // namespace internal
}  // namespace tapa
