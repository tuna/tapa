// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef FPGA_RUNTIME_XILINX_ENVIRON_H_
#define FPGA_RUNTIME_XILINX_ENVIRON_H_

#include <string>
#include <unordered_map>

namespace fpga::xilinx {

using Environ = std::unordered_map<std::string, std::string>;

Environ GetEnviron();

}  // namespace fpga::xilinx

#endif  // FPGA_RUNTIME_XILINX_ENVIRON_H_
