// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "frt/devices/opencl_device.h"

#include <algorithm>
#include <string>
#include <vector>

#include <glog/logging.h>
#include <CL/cl2.hpp>

#include "frt/devices/opencl_device_matcher.h"
#include "frt/devices/opencl_util.h"

namespace fpga {
namespace internal {

namespace {

template <cl_int name>
int64_t GetTime(const cl::Event& event) {
  cl_int err;
  int64_t time = event.getProfilingInfo<name>(&err);
  CL_CHECK(err);
  return time;
}

template <cl_int name>
int64_t Earliest(const std::vector<cl::Event>& events,
                 int64_t default_value = 0) {
  if (!events.empty()) {
    default_value = std::numeric_limits<int64_t>::max();
    for (auto& event : events)
      default_value = std::min(default_value, GetTime<name>(event));
  }
  return default_value;
}

template <cl_int name>
int64_t Latest(const std::vector<cl::Event>& events,
               int64_t default_value = 0) {
  if (!events.empty()) {
    default_value = std::numeric_limits<int64_t>::min();
    for (auto& event : events)
      default_value = std::max(default_value, GetTime<name>(event));
  }
  return default_value;
}

}  // namespace

void OpenclDevice::SetScalarArg(size_t index, const void* arg, int size) {
  auto pair = GetKernel(index);
  pair.second.setArg(pair.first, size, arg);
}

void OpenclDevice::SetBufferArg(size_t index, Tag tag, const BufferArg& arg) {
  cl_mem_flags flags = 0;
  switch (tag) {
    case Tag::kPlaceHolder:
      break;
    case Tag::kReadOnly:
      flags = CL_MEM_READ_ONLY;
      break;
    case Tag::kWriteOnly:
      flags = CL_MEM_WRITE_ONLY;
      break;
    case Tag::kReadWrite:
      flags = CL_MEM_READ_WRITE;
      break;
  }
  cl::Buffer buffer = CreateBuffer(index, flags, arg.Get(), arg.SizeInBytes());
  if (tag == Tag::kReadOnly || tag == Tag::kReadWrite) {
    store_indices_.insert(index);
  }
  if (tag == Tag::kWriteOnly || tag == Tag::kReadWrite) {
    load_indices_.insert(index);
  }
  auto pair = GetKernel(index);
  pair.second.setArg(pair.first, buffer);
}

size_t OpenclDevice::SuspendBuffer(size_t index) {
  return load_indices_.erase(index) + store_indices_.erase(index);
}

void OpenclDevice::Exec() {
  compute_event_.resize(kernels_.size());
  size_t i = 0;
  for (auto& [k, kernel] : kernels_) {
    CL_CHECK(cmd_.enqueueNDRangeKernel(kernel, cl::NullRange, cl::NDRange(1),
                                       cl::NDRange(1), &load_event_,
                                       &compute_event_[i++]));
  }
  is_finished_ = false;
}

void OpenclDevice::Finish() {
  CL_CHECK(cmd_.flush());
  CL_CHECK(cmd_.finish());
  is_finished_ = true;
}

void OpenclDevice::Kill() { LOG(ERROR) << "OpenCl kernels cannot be killed"; }

bool OpenclDevice::IsFinished() const {
  if (!is_finished_) {
    LOG(ERROR) << "Not implemented, assuming to be running.";
  }
  return is_finished_;
}

std::vector<ArgInfo> OpenclDevice::GetArgsInfo() const {
  std::vector<ArgInfo> args;
  args.reserve(arg_table_.size());
  for (const auto& [k, v] : arg_table_) args.push_back(v);
  std::sort(args.begin(), args.end(), [](const ArgInfo& a, const ArgInfo& b) {
    return a.index < b.index;
  });
  return args;
}

int64_t OpenclDevice::LoadTimeNanoSeconds() const {
  return Latest<CL_PROFILING_COMMAND_END>(load_event_) -
         Earliest<CL_PROFILING_COMMAND_START>(load_event_);
}
int64_t OpenclDevice::ComputeTimeNanoSeconds() const {
  return Latest<CL_PROFILING_COMMAND_END>(compute_event_) -
         Earliest<CL_PROFILING_COMMAND_START>(compute_event_);
}
int64_t OpenclDevice::StoreTimeNanoSeconds() const {
  return Latest<CL_PROFILING_COMMAND_END>(store_event_) -
         Earliest<CL_PROFILING_COMMAND_START>(store_event_);
}
size_t OpenclDevice::LoadBytes() const {
  size_t total = 0;
  cl_int err;
  for (const auto& buf : GetLoadBuffers()) {
    total += buf.getInfo<CL_MEM_SIZE>(&err);
    CL_CHECK(err);
  }
  return total;
}
size_t OpenclDevice::StoreBytes() const {
  size_t total = 0;
  cl_int err;
  for (const auto& buf : GetStoreBuffers()) {
    total += buf.getInfo<CL_MEM_SIZE>(&err);
    CL_CHECK(err);
  }
  return total;
}

void OpenclDevice::Initialize(const cl::Program::Binaries& binaries,
                              const std::string& vendor_name,
                              const OpenclDeviceMatcher& device_matcher,
                              const std::vector<std::string>& kernel_names,
                              const std::vector<int>& kernel_arg_counts) {
  std::vector<cl::Platform> platforms;
  CL_CHECK(cl::Platform::get(&platforms));
  cl_int err;
  for (const auto& platform : platforms) {
    std::string platformName = platform.getInfo<CL_PLATFORM_NAME>(&err);
    CL_CHECK(err);
    LOG(INFO) << "Found platform: " << platformName;
    if (platformName == vendor_name) {
      std::vector<cl::Device> devices;
      CL_CHECK(platform.getDevices(CL_DEVICE_TYPE_ACCELERATOR, &devices));
      for (const auto& device : devices) {
        if (std::string device_name = device_matcher.Match(device);
            !device_name.empty()) {
          LOG(INFO) << "Using " << device_name;
          device_ = device;
          context_ = cl::Context(device, nullptr, nullptr, nullptr, &err);
          if (err == CL_DEVICE_NOT_AVAILABLE) {
            LOG(WARNING) << "Device '" << device_name << "' not available";
            continue;
          }
          CL_CHECK(err);
          cmd_ = cl::CommandQueue(context_, device,
                                  CL_QUEUE_OUT_OF_ORDER_EXEC_MODE_ENABLE |
                                      CL_QUEUE_PROFILING_ENABLE,
                                  &err);
          CL_CHECK(err);
          std::vector<int> binary_status;
          program_ =
              cl::Program(context_, {device}, binaries, &binary_status, &err);
          for (auto s : binary_status) CL_CHECK(s);
          CL_CHECK(err);
          CL_CHECK(program_.build());
          for (size_t i = 0; i < kernel_names.size(); ++i) {
            kernels_[kernel_arg_counts[i]] =
                cl::Kernel(program_, kernel_names[i].c_str(), &err);
            CL_CHECK(err);
          }
          return;
        }
      }
      LOG(FATAL) << "Target device '" << device_matcher.GetTargetName()
                 << "' not found";
    }
  }
  LOG(FATAL) << "Target platform '" + vendor_name + "' not found";
}

cl::Buffer OpenclDevice::CreateBuffer(size_t index, cl_mem_flags flags,
                                      void* host_ptr, size_t size) {
  cl_int err;
  auto buffer = cl::Buffer(context_, flags, size, host_ptr, &err);
  CL_CHECK(err);
  buffer_table_[index] = buffer;
  return buffer;
}

std::vector<cl::Memory> OpenclDevice::GetLoadBuffers() const {
  std::vector<cl::Memory> bufs;
  bufs.reserve(load_indices_.size());
  for (auto idx : load_indices_) bufs.push_back(buffer_table_.at(idx));
  return bufs;
}

std::vector<cl::Memory> OpenclDevice::GetStoreBuffers() const {
  std::vector<cl::Memory> bufs;
  bufs.reserve(store_indices_.size());
  for (auto idx : store_indices_) bufs.push_back(buffer_table_.at(idx));
  return bufs;
}

std::pair<int, cl::Kernel> OpenclDevice::GetKernel(size_t index) const {
  auto it = std::prev(kernels_.upper_bound(index));
  return {index - it->first, it->second};
}

}  // namespace internal
}  // namespace fpga
