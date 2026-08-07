#include "tapa/host/frt/instance.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <memory>
#include <string>

#include <glog/logging.h>

#include "tapa/host/frt/c_api.h"
#include "tapa/host/frt/types.h"

namespace tapa {
namespace internal {
namespace frt {

// Flag ownership lives entirely in C++ (flags.cpp). These forwarders are
// defined there; referencing them keeps flags.cpp linked so its gflags
// `DEFINE_*` registrations survive.
void ForwardFlagsToEnv(const std::string& bitstream);
const char* SimulatorFlag(const std::string& bitstream);
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
  void* handle = nullptr;

  explicit Impl(const std::string& bitstream) {
    if (bitstream.empty()) return;
    ForwardFlagsToEnv(bitstream);
    handle = frt_instance_open(bitstream.c_str(), SimulatorFlag(bitstream));
    LOG_IF(FATAL, handle == nullptr)
        << "failed to open '" << bitstream << "': " << LastErr();
  }

  ~Impl() {
    if (handle) frt_instance_close(handle);
  }
};

Instance::Instance(const std::string& bitstream)
    : impl_(std::make_unique<Impl>(bitstream)) {}

Instance::Instance(Instance&&) noexcept = default;
Instance& Instance::operator=(Instance&&) noexcept = default;
Instance::~Instance() = default;

void Instance::SetScalarArg(int index, const void* data, size_t size) {
  if (!impl_->handle) return;
  CheckFfi(
      frt_instance_set_scalar_bytes(
          impl_->handle, index, reinterpret_cast<const uint8_t*>(data), size),
      "set_scalar");
}

void Instance::SetBufferArg(int index, const void* ptr, size_t bytes, Tag tag) {
  if (!impl_->handle) return;
  // The FRT C ABI takes a mutable pointer. Read-only mmaps expose a const
  // element pointer, so const-cast here as the former BufferArg did; the
  // read/write direction is conveyed by `tag`, not pointer constness.
  CheckFfi(frt_instance_set_buffer_arg_typed(
               impl_->handle, index,
               const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(ptr)),
               bytes, static_cast<int>(tag)),
           "set_buffer");
}

void Instance::SetStreamArg(int index, const std::string& path) {
  if (!impl_->handle) return;
  CheckFfi(frt_instance_set_stream_arg(impl_->handle, index, path.c_str()),
           "set_stream");
}

void Instance::WriteToDevice() {
  if (impl_->handle)
    CheckFfi(frt_instance_write_to_device(impl_->handle), "write_to_device");
}

void Instance::ReadFromDevice() {
  if (impl_->handle)
    CheckFfi(frt_instance_read_from_device(impl_->handle), "read_from_device");
}

void Instance::Exec() {
  if (impl_->handle) CheckFfi(frt_instance_exec(impl_->handle), "exec");
}

void Instance::Finish() {
  if (impl_->handle) CheckFfi(frt_instance_finish(impl_->handle), "finish");
  if (CosimSetupOnly()) std::exit(0);
}

void Instance::Kill() {
  if (impl_->handle) CheckFfi(frt_instance_kill(impl_->handle), "kill");
}

bool Instance::IsFinished() const {
  if (!impl_->handle) return true;
  int ret = frt_instance_is_finished(impl_->handle);
  CHECK_GE(ret, 0) << "is_finished failed: " << LastErr();
  return ret != 0;
}

int64_t Instance::LoadTimeNanoSeconds() const {
  return impl_->handle ? frt_instance_load_ns(impl_->handle) : 0;
}
int64_t Instance::ComputeTimeNanoSeconds() const {
  return impl_->handle ? frt_instance_compute_ns(impl_->handle) : 0;
}
int64_t Instance::StoreTimeNanoSeconds() const {
  return impl_->handle ? frt_instance_store_ns(impl_->handle) : 0;
}
double Instance::LoadTimeSeconds() const {
  return LoadTimeNanoSeconds() * 1e-9;
}
double Instance::ComputeTimeSeconds() const {
  return ComputeTimeNanoSeconds() * 1e-9;
}
double Instance::StoreTimeSeconds() const {
  return StoreTimeNanoSeconds() * 1e-9;
}

}  // namespace frt
}  // namespace internal
}  // namespace tapa
