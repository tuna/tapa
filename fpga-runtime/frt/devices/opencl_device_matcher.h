// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef FPGA_RUNTIME_OPENCL_DEVICE_MATCHER_H_
#define FPGA_RUNTIME_OPENCL_DEVICE_MATCHER_H_

#include <string>

#include <CL/cl2.hpp>

namespace fpga {
namespace internal {

class OpenclDeviceMatcher {
 public:
  virtual std::string GetTargetName() const = 0;
  // Returns matched device name, or empty string if no match.
  virtual std::string Match(cl::Device device) const = 0;
};

}  // namespace internal
}  // namespace fpga

#endif  // FPGA_RUNTIME_OPENCL_DEVICE_MATCHER_H_
