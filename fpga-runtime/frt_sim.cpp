// Simulation-only stub for fpga::Instance.
// This provides the Instance API without OpenCL/XRT dependencies.
// All device methods LOG(FATAL) since they should never be called
// during software simulation (bitstream is empty).

#include "frt.h"

#include <glog/logging.h>

namespace fpga {

Instance::Instance(const std::string& bitstream) {
  LOG(FATAL) << "fpga::Instance is not supported in simulation-only mode. "
             << "Software simulation should use an empty bitstream path.";
}

size_t Instance::SuspendBuf(int index) {
  LOG(FATAL) << "not supported in simulation-only mode";
}

void Instance::WriteToDevice() {
  LOG(FATAL) << "not supported in simulation-only mode";
}

void Instance::ReadFromDevice() {
  LOG(FATAL) << "not supported in simulation-only mode";
}

void Instance::Exec() { LOG(FATAL) << "not supported in simulation-only mode"; }

void Instance::Finish() {
  LOG(FATAL) << "not supported in simulation-only mode";
}

void Instance::Kill() { LOG(FATAL) << "not supported in simulation-only mode"; }

bool Instance::IsFinished() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

std::vector<ArgInfo> Instance::GetArgsInfo() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

int64_t Instance::LoadTimeNanoSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

int64_t Instance::ComputeTimeNanoSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

int64_t Instance::StoreTimeNanoSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

double Instance::LoadTimeSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

double Instance::ComputeTimeSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

double Instance::StoreTimeSeconds() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

double Instance::LoadThroughputGbps() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

double Instance::StoreThroughputGbps() const {
  LOG(FATAL) << "not supported in simulation-only mode";
}

void Instance::ConditionallyFinish(bool has_stream) {
  LOG(FATAL) << "not supported in simulation-only mode";
}

}  // namespace fpga
