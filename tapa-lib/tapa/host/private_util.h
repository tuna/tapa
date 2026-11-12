// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

// NOTE: This is a private header that is not exported for packaging.

#include <functional>
#include <istream>
#include <string>
#include <string_view>

namespace tapa::internal {

std::string StrCat(std::initializer_list<std::string_view> pieces);

// The physical cores a `/proc/cpuinfo` listing names among the processors
// `allowed` admits, each a (package, core id) pair; 0 if it names none.
int count_physical_cores(std::istream& cpuinfo,
                         const std::function<bool(int)>& allowed);

// The physical cores this process may run on: under `taskset`, a container's
// cpuset or a batch scheduler's allocation, only the cores it was given.
int get_physical_core_count();

}  // namespace tapa::internal
