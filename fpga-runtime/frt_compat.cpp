// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#include "frt.h"

#include <algorithm>
#include <any>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#include <gflags/gflags.h>
#include <glog/logging.h>

DEFINE_bool(xosim_start_gui, false,
            "deprecated: GUI launch control is handled in Rust runtime");
DEFINE_bool(xosim_save_waveform, false, "save waveform in the work directory");
DEFINE_string(xosim_work_dir, "",
              "deprecated: work dir is managed by Rust runtime");
DEFINE_bool(xosim_work_dir_parallel_cosim, false,
            "deprecated: parallel work dir is managed by Rust runtime");
DEFINE_string(xosim_executable, "",
              "deprecated: fast cosim is linked in-process via libfrt");
DEFINE_string(xosim_part_num, "",
              "deprecated: part number is taken from kernel metadata");
DEFINE_string(xosim_simulator, "",
              "simulator backend to use: 'xsim' (default) or 'verilator'");
DEFINE_bool(xosim_setup_only, false, "only setup the simulation");
DEFINE_bool(xosim_resume_from_post_sim, false,
            "skip simulation and do post-sim checking");
DEFINE_string(xocl_bdf, "",
              "if not empty, use the specified PCIe Bus:Device:Function for "
              "XRT/OpenCL device selection");

extern "C" {
void* frt_instance_open(const char* path, const char* simulator);
void frt_instance_close(void* handle);
const char* frt_last_error_message();
int frt_instance_set_scalar_bytes(void* handle, uint32_t index,
                                  const uint8_t* value, size_t size);
int frt_instance_set_buffer_arg(void* handle, uint32_t index, uint8_t* ptr,
                                size_t bytes);
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

const char* SimulatorFlagForPath(const std::string& bitstream) {
  if (!IsCosimPackage(bitstream)) return nullptr;
  if (FLAGS_xosim_simulator.empty()) return "xsim";
  return FLAGS_xosim_simulator.c_str();
}

int64_t NsSince(const std::chrono::steady_clock::time_point& tic) {
  return std::chrono::duration_cast<std::chrono::nanoseconds>(
             std::chrono::steady_clock::now() - tic)
      .count();
}

void SetOrUnsetEnv(const char* name, const std::string& value) {
  if (value.empty()) {
    unsetenv(name);
  } else {
    setenv(name, value.c_str(), 1);
  }
}

void SetBoolEnv(const char* name, bool value) {
  setenv(name, value ? "1" : "0", 1);
}

void ConfigureRustCosimEnv(const std::string& bitstream) {
  if (!IsCosimPackage(bitstream)) {
    unsetenv("FRT_XOSIM_START_GUI");
    unsetenv("FRT_XOSIM_SAVE_WAVEFORM");
    unsetenv("FRT_XOSIM_WORK_DIR");
    unsetenv("FRT_XOSIM_WORK_DIR_PARALLEL");
    unsetenv("FRT_XOSIM_PART_NUM");
    unsetenv("FRT_XOSIM_SETUP_ONLY");
    unsetenv("FRT_XOSIM_RESUME_FROM_POST_SIM");
    return;
  }
  SetBoolEnv("FRT_XOSIM_START_GUI", FLAGS_xosim_start_gui);
  SetBoolEnv("FRT_XOSIM_SAVE_WAVEFORM", FLAGS_xosim_save_waveform);
  SetOrUnsetEnv("FRT_XOSIM_WORK_DIR", FLAGS_xosim_work_dir);
  SetBoolEnv("FRT_XOSIM_WORK_DIR_PARALLEL",
             FLAGS_xosim_work_dir_parallel_cosim);
  SetOrUnsetEnv("FRT_XOSIM_PART_NUM", FLAGS_xosim_part_num);
  SetBoolEnv("FRT_XOSIM_SETUP_ONLY", FLAGS_xosim_setup_only);
  SetBoolEnv("FRT_XOSIM_RESUME_FROM_POST_SIM",
             FLAGS_xosim_resume_from_post_sim);
}

void ConfigureRustRuntimeEnv(const std::string& bitstream) {
  SetOrUnsetEnv("FRT_XOCL_BDF", FLAGS_xocl_bdf);
  ConfigureRustCosimEnv(bitstream);
}

}  // namespace

struct Instance::Impl {
  explicit Impl(std::string bitstream_path)
      : bitstream(std::move(bitstream_path)),
        created(std::chrono::steady_clock::now()) {
    if (!bitstream.empty()) {
      ConfigureRustRuntimeEnv(bitstream);
      handle =
          frt_instance_open(bitstream.c_str(), SimulatorFlagForPath(bitstream));
      if (handle == nullptr) {
        LOG(FATAL) << "failed to open bitstream '" << bitstream
                   << "' via Rust runtime: " << LastErr();
      }
    }
  }

  ~Impl() {
    if (handle != nullptr) {
      frt_instance_close(handle);
      handle = nullptr;
    }
  }

  std::string bitstream;
  void* handle = nullptr;
  std::unordered_map<int, std::vector<char>> scalar_args;
  std::unordered_map<int, std::pair<internal::Tag, internal::BufferArg>>
      buffer_args;
  std::unordered_map<int, internal::StreamArg*> stream_args;
  std::chrono::steady_clock::time_point created;
  int64_t load_ns = 0;
  int64_t compute_ns = 0;
  int64_t store_ns = 0;
  size_t load_bytes = 0;
  size_t store_bytes = 0;
  bool finished = false;
};

Instance::Instance(const std::string& bitstream)
    : impl_(std::make_unique<Impl>(bitstream)) {}

Instance::Instance(Instance&&) noexcept = default;
Instance& Instance::operator=(Instance&&) noexcept = default;
Instance::~Instance() = default;

void Instance::SetScalarArgRaw(int index, const void* arg, size_t size) {
  auto& vec = impl_->scalar_args[index];
  vec.resize(size);
  std::memcpy(vec.data(), arg, size);
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_set_scalar_bytes(
                 impl_->handle, static_cast<uint32_t>(index),
                 reinterpret_cast<const uint8_t*>(arg), size),
             "frt_instance_set_scalar_bytes");
  }
}

void Instance::SetBufferArgRaw(int index, internal::Tag tag,
                               internal::BufferArg arg) {
  impl_->buffer_args[index] = std::make_pair(tag, arg);
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_set_buffer_arg_typed(
                 impl_->handle, static_cast<uint32_t>(index),
                 reinterpret_cast<uint8_t*>(arg.Get()), arg.SizeInBytes(),
                 static_cast<int>(tag)),
             "frt_instance_set_buffer_arg_typed");
  }
}

void Instance::SetStreamArgRaw(int index, internal::Tag,
                               internal::StreamArg& arg) {
  impl_->stream_args[index] = &arg;
  if (impl_->handle != nullptr) {
    std::string shm_path;
    try {
      shm_path = arg.get<internal::StreamFfiContext>().path;
    } catch (const std::bad_any_cast&) {
      // Keep empty path for non-shared-memory stream contexts.
    }
    CheckFfi(frt_instance_set_stream_arg(
                 impl_->handle, static_cast<uint32_t>(index), shm_path.c_str()),
             "frt_instance_set_stream_arg");
  }
}

size_t Instance::SuspendBuf(int index) {
  size_t erased = impl_->buffer_args.erase(index);
  if (impl_->handle != nullptr) {
    erased = frt_instance_suspend_buffer(impl_->handle,
                                         static_cast<uint32_t>(index));
  }
  return erased;
}

void Instance::WriteToDevice() {
  auto tic = std::chrono::steady_clock::now();
  impl_->load_bytes = 0;
  for (const auto& [_, tagged] : impl_->buffer_args) {
    if (tagged.first == internal::Tag::kReadOnly ||
        tagged.first == internal::Tag::kReadWrite) {
      impl_->load_bytes += tagged.second.SizeInBytes();
    }
  }
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_write_to_device(impl_->handle),
             "frt_instance_write_to_device");
    impl_->load_ns = static_cast<int64_t>(frt_instance_load_ns(impl_->handle));
  } else {
    impl_->load_ns = std::max<int64_t>(1, NsSince(tic));
  }
}

void Instance::ReadFromDevice() {
  auto tic = std::chrono::steady_clock::now();
  impl_->store_bytes = 0;
  for (const auto& [_, tagged] : impl_->buffer_args) {
    if (tagged.first == internal::Tag::kWriteOnly ||
        tagged.first == internal::Tag::kReadWrite) {
      impl_->store_bytes += tagged.second.SizeInBytes();
    }
  }
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_read_from_device(impl_->handle),
             "frt_instance_read_from_device");
    impl_->store_ns =
        static_cast<int64_t>(frt_instance_store_ns(impl_->handle));
  } else {
    impl_->store_ns = std::max<int64_t>(1, NsSince(tic));
  }
}

void Instance::Exec() {
  auto tic = std::chrono::steady_clock::now();
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_exec(impl_->handle), "frt_instance_exec");
    impl_->compute_ns =
        static_cast<int64_t>(frt_instance_compute_ns(impl_->handle));
  } else {
    impl_->compute_ns = std::max<int64_t>(1, NsSince(tic));
    impl_->finished = true;
  }
}

void Instance::Finish() {
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_finish(impl_->handle), "frt_instance_finish");
    impl_->compute_ns =
        static_cast<int64_t>(frt_instance_compute_ns(impl_->handle));
    impl_->store_ns =
        static_cast<int64_t>(frt_instance_store_ns(impl_->handle));
  }
  impl_->finished = true;
  if (FLAGS_xosim_setup_only) {
    std::exit(0);
  }
}

void Instance::Kill() {
  if (impl_->handle != nullptr) {
    CheckFfi(frt_instance_kill(impl_->handle), "frt_instance_kill");
  }
  impl_->finished = true;
}

bool Instance::IsFinished() const {
  if (impl_->handle != nullptr) {
    int ret = frt_instance_is_finished(impl_->handle);
    CHECK_GE(ret, 0) << "frt_instance_is_finished failed: " << LastErr();
    return ret != 0;
  }
  return impl_->finished;
}

std::vector<ArgInfo> Instance::GetArgsInfo() const {
  if (impl_->handle != nullptr) {
    uint32_t count = 0;
    CheckFfi(frt_instance_get_arg_count(impl_->handle, &count),
             "frt_instance_get_arg_count");
    std::vector<ArgInfo> args;
    args.reserve(count);
    for (uint32_t i = 0; i < count; ++i) {
      uint32_t index = 0;
      int cat = 0;
      const char* name = nullptr;
      const char* type = nullptr;
      CheckFfi(
          frt_instance_get_arg(impl_->handle, i, &index, &cat, &name, &type),
          "frt_instance_get_arg");
      ArgInfo arg;
      arg.index = static_cast<int>(index);
      arg.name = name == nullptr ? "" : name;
      arg.type = type == nullptr ? "" : type;
      if (cat < static_cast<int>(ArgInfo::kScalar) ||
          cat > static_cast<int>(ArgInfo::kStreams)) {
        arg.cat = ArgInfo::kScalar;
      } else {
        arg.cat = static_cast<ArgInfo::Cat>(cat);
      }
      args.push_back(std::move(arg));
    }
    std::sort(args.begin(), args.end(), [](const ArgInfo& a, const ArgInfo& b) {
      return a.index < b.index;
    });
    return args;
  }

  std::vector<ArgInfo> args;
  args.reserve(impl_->scalar_args.size() + impl_->buffer_args.size() +
               impl_->stream_args.size());
  for (const auto& [idx, _] : impl_->scalar_args)
    args.push_back(
        {idx, "scalar_" + std::to_string(idx), "scalar", ArgInfo::kScalar});
  for (const auto& [idx, _] : impl_->buffer_args)
    args.push_back(
        {idx, "mmap_" + std::to_string(idx), "mmap", ArgInfo::kMmap});
  for (const auto& [idx, _] : impl_->stream_args)
    args.push_back(
        {idx, "stream_" + std::to_string(idx), "stream", ArgInfo::kStream});
  std::sort(args.begin(), args.end(), [](const ArgInfo& a, const ArgInfo& b) {
    return a.index < b.index;
  });
  return args;
}

int64_t Instance::LoadTimeNanoSeconds() const { return impl_->load_ns; }
int64_t Instance::ComputeTimeNanoSeconds() const { return impl_->compute_ns; }
int64_t Instance::StoreTimeNanoSeconds() const { return impl_->store_ns; }

double Instance::LoadTimeSeconds() const {
  return static_cast<double>(LoadTimeNanoSeconds()) * 1e-9;
}
double Instance::ComputeTimeSeconds() const {
  return static_cast<double>(ComputeTimeNanoSeconds()) * 1e-9;
}
double Instance::StoreTimeSeconds() const {
  return static_cast<double>(StoreTimeNanoSeconds()) * 1e-9;
}

double Instance::LoadThroughputGbps() const {
  return impl_->load_ns == 0
             ? 0.0
             : static_cast<double>(impl_->load_bytes) / impl_->load_ns;
}
double Instance::StoreThroughputGbps() const {
  return impl_->store_ns == 0
             ? 0.0
             : static_cast<double>(impl_->store_bytes) / impl_->store_ns;
}

void Instance::ConditionallyFinish(bool has_stream) {
  if (!has_stream) Finish();
}

}  // namespace fpga
