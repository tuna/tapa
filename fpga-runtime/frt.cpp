// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "frt.h"

#include <fstream>
#include <string>

#include <glog/logging.h>
#include <CL/cl2.hpp>

#include "frt/devices/intel_opencl_device.h"
#include "frt/devices/tapa_fast_cosim_device.h"
#include "frt/devices/xilinx_opencl_device.h"

namespace fpga {

Instance::Instance(const std::string& bitstream) {
  LOG(INFO) << "Loading " << bitstream;
  std::ifstream stream(bitstream, std::ios::binary);
  const cl::Program::Binaries binaries = {
      {std::istreambuf_iterator<char>(stream),
       std::istreambuf_iterator<char>()}};

  if ((device_ = internal::XilinxOpenclDevice::New(binaries))) return;
  if ((device_ = internal::IntelOpenclDevice::New(binaries))) return;
  if ((device_ = internal::TapaFastCosimDevice::New(
           bitstream,
           std::string_view(reinterpret_cast<const char*>(binaries[0].data()),
                            binaries[0].size())))) {
    return;
  }

  LOG(FATAL) << "Unexpected bitstream file";
}

size_t Instance::SuspendBuf(int index) { return device_->SuspendBuffer(index); }

void Instance::WriteToDevice() { device_->WriteToDevice(); }

void Instance::ReadFromDevice() { device_->ReadFromDevice(); }

void Instance::Exec() { device_->Exec(); }

void Instance::Finish() { device_->Finish(); }

void Instance::Kill() { device_->Kill(); }

bool Instance::IsFinished() const { return device_->IsFinished(); }

std::vector<ArgInfo> Instance::GetArgsInfo() const {
  return device_->GetArgsInfo();
}

int64_t Instance::LoadTimeNanoSeconds() const {
  return device_->LoadTimeNanoSeconds();
}

int64_t Instance::ComputeTimeNanoSeconds() const {
  return device_->ComputeTimeNanoSeconds();
}

int64_t Instance::StoreTimeNanoSeconds() const {
  return device_->StoreTimeNanoSeconds();
}

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
  return static_cast<double>(device_->LoadBytes()) /
         static_cast<double>(LoadTimeNanoSeconds());
}

double Instance::StoreThroughputGbps() const {
  return static_cast<double>(device_->StoreBytes()) /
         static_cast<double>(StoreTimeNanoSeconds());
}

void Instance::ConditionallyFinish(bool has_stream) {
  if (!has_stream) {
    VLOG(1) << "no stream found; waiting for command to finish";
    Finish();
  }
}

}  // namespace fpga
