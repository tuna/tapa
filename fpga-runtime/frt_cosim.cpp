// Cosim-only fpga::Instance for macOS (no OpenCL/XRT dependencies).
// Supports only TapaFastCosimDevice for Verilator-based cosimulation.

#include "frt.h"

#include <fstream>
#include <string>

#include <glog/logging.h>

#include "frt/devices/tapa_fast_cosim_device.h"

namespace fpga {

Instance::Instance(const std::string& bitstream) {
  LOG(INFO) << "Loading " << bitstream;
  std::ifstream stream(bitstream, std::ios::binary);
  std::string content((std::istreambuf_iterator<char>(stream)),
                      std::istreambuf_iterator<char>());

  if ((device_ = internal::TapaFastCosimDevice::New(
           bitstream, std::string_view(content.data(), content.size())))) {
    return;
  }

  LOG(FATAL) << "Unexpected bitstream file: " << bitstream
             << " (only cosim is supported on macOS)";
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
  if (!has_stream) Finish();
}

}  // namespace fpga
