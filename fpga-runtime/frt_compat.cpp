// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#include "frt.h"

#include <any>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>

#include <gflags/gflags.h>
#include <glog/logging.h>

DEFINE_bool(xsim_start_gui, false,
            "open the Vivado GUI for interactive xsim debugging");
DEFINE_bool(xsim_save_waveform, false,
            "save xsim waveform output in the work directory");
DEFINE_string(cosim_work_dir, "",
              "if not empty, keep cosim artifacts in the specified directory");
DEFINE_bool(cosim_work_dir_parallel, false,
            "create a unique work directory per concurrent cosim instance");
DEFINE_string(cosim_executable, "",
              "deprecated: fast cosim is linked in-process via libfrt");
DEFINE_string(xsim_part_num, "",
              "if not empty, override the FPGA part number for xsim");
DEFINE_string(cosim_simulator, "",
              "simulator backend to use: 'xsim' (default) or 'verilator'");
DEFINE_bool(cosim_setup_only, false,
            "generate the cosim work directory but do not run the simulator");
DEFINE_bool(cosim_resume_from_post_sim, false,
            "skip re-running the simulator and execute only post-sim checks");
DEFINE_string(xocl_bdf, "",
              "if not empty, use the specified PCIe Bus:Device:Function for "
              "XRT/OpenCL device selection");

extern "C" {
void* frt_instance_open(const char* path, const char* simulator);
void frt_instance_close(void* handle);
const char* frt_last_error_message();
int frt_instance_set_scalar_bytes(void* handle, uint32_t index,
                                  const uint8_t* value, size_t size);
int frt_instance_set_buffer_arg_typed(void* handle, uint32_t index,
                                      uint8_t* ptr, size_t bytes, int tag);
int frt_instance_set_stream_arg(void* handle, uint32_t index,
                                const char* shm_path);
size_t frt_instance_suspend_buffer(void* handle, uint32_t index);
int frt_instance_get_arg_count(void* handle, uint32_t* out_count);
int frt_instance_get_arg(void* handle, uint32_t ordinal, uint32_t* out_index,
                         int* out_cat, const char** out_name,
                         const char** out_type);
int frt_instance_write_to_device(void* handle);
int frt_instance_read_from_device(void* handle);
int frt_instance_exec(void* handle);
int frt_instance_pause(void* handle);
int frt_instance_resume(void* handle);
int frt_instance_finish(void* handle);
int frt_instance_kill(void* handle);
int frt_instance_is_finished(void* handle);
uint64_t frt_instance_load_ns(void* handle);
uint64_t frt_instance_compute_ns(void* handle);
uint64_t frt_instance_store_ns(void* handle);
}  // extern "C"

namespace fpga {

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

bool IsCosimPackage(const std::string& path) {
  return (path.size() > 3 && path.compare(path.size() - 3, 3, ".xo") == 0) ||
         (path.size() > 4 && path.compare(path.size() - 4, 4, ".zip") == 0);
}

const char* SimulatorFlag(const std::string& bitstream) {
  if (!IsCosimPackage(bitstream)) return nullptr;
  return FLAGS_cosim_simulator.empty() ? "xsim" : FLAGS_cosim_simulator.c_str();
}

void SetEnvIf(const char* name, const std::string& val) {
  if (!val.empty()) {
    setenv(name, val.c_str(), 1);
  }
}

void SetBoolEnvIf(const char* name, bool val) {
  // Only set the env var when the flag is true; when false, preserve any
  // user-provided env var instead of silently clearing it.
  if (val) {
    setenv(name, "1", 1);
  }
}

void ForwardFlagsToEnv(const std::string& bitstream) {
  SetEnvIf("FRT_XOCL_BDF", FLAGS_xocl_bdf);
  if (!IsCosimPackage(bitstream)) return;
  SetBoolEnvIf("FRT_XSIM_START_GUI", FLAGS_xsim_start_gui);
  SetBoolEnvIf("FRT_XSIM_SAVE_WAVEFORM", FLAGS_xsim_save_waveform);
  SetEnvIf("FRT_COSIM_WORK_DIR", FLAGS_cosim_work_dir);
  SetBoolEnvIf("FRT_COSIM_WORK_DIR_PARALLEL", FLAGS_cosim_work_dir_parallel);
  SetEnvIf("FRT_XSIM_PART_NUM", FLAGS_xsim_part_num);
  SetBoolEnvIf("FRT_COSIM_SETUP_ONLY", FLAGS_cosim_setup_only);
  SetBoolEnvIf("FRT_COSIM_RESUME_FROM_POST_SIM",
               FLAGS_cosim_resume_from_post_sim);
}

}  // namespace

namespace fpga::internal {

void ForwardFlagsToEnvForTest(const std::string& bitstream) {
  ForwardFlagsToEnv(bitstream);
}

}  // namespace fpga::internal

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

void Instance::SetScalarArgRaw(int index, const void* arg, size_t size) {
  if (!impl_->handle) return;
  CheckFfi(
      frt_instance_set_scalar_bytes(
          impl_->handle, index, reinterpret_cast<const uint8_t*>(arg), size),
      "set_scalar");
}

void Instance::SetBufferArgRaw(int index, internal::Tag tag,
                               internal::BufferArg arg) {
  if (!impl_->handle) return;
  CheckFfi(frt_instance_set_buffer_arg_typed(
               impl_->handle, index, reinterpret_cast<uint8_t*>(arg.Get()),
               arg.SizeInBytes(), static_cast<int>(tag)),
           "set_buffer");
}

void Instance::SetStreamArgRaw(int index, internal::Tag,
                               internal::StreamArg& arg) {
  if (!impl_->handle) return;
  std::string path;
  try {
    path = arg.get<internal::StreamFfiContext>().path;
  } catch (const std::bad_any_cast&) {
  }
  CheckFfi(frt_instance_set_stream_arg(impl_->handle, index, path.c_str()),
           "set_stream");
}

size_t Instance::SuspendBuf(int index) {
  return impl_->handle ? frt_instance_suspend_buffer(impl_->handle, index) : 0;
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

void Instance::Pause() {
  if (impl_->handle) CheckFfi(frt_instance_pause(impl_->handle), "pause");
}

void Instance::Resume() {
  if (impl_->handle) CheckFfi(frt_instance_resume(impl_->handle), "resume");
}

void Instance::Finish() {
  if (impl_->handle) CheckFfi(frt_instance_finish(impl_->handle), "finish");
  if (FLAGS_cosim_setup_only) std::exit(0);
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

std::vector<ArgInfo> Instance::GetArgsInfo() const {
  if (!impl_->handle) return {};
  uint32_t count = 0;
  CheckFfi(frt_instance_get_arg_count(impl_->handle, &count), "get_arg_count");
  std::vector<ArgInfo> args;
  for (uint32_t i = 0; i < count; ++i) {
    uint32_t idx = 0;
    int cat = 0;
    const char* name = nullptr;
    const char* type = nullptr;
    CheckFfi(frt_instance_get_arg(impl_->handle, i, &idx, &cat, &name, &type),
             "get_arg");
    args.push_back({static_cast<int>(idx), name ? name : "", type ? type : "",
                    static_cast<ArgInfo::Cat>(cat)});
  }
  return args;
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
double Instance::LoadThroughputGbps() const { return 0.0; }
double Instance::StoreThroughputGbps() const { return 0.0; }

void Instance::ConditionallyFinish(bool has_stream) {
  if (!has_stream) Finish();
}

}  // namespace fpga
