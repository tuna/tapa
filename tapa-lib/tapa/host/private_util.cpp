// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/private_util.h"

#include <set>
#include <sstream>
#include <string>
#include <string_view>
#include <thread>
#include <utility>

#ifdef __APPLE__
#include <sys/sysctl.h>
#else
#include <sched.h>

#include <fstream>
#endif

namespace tapa::internal {

std::string StrCat(std::initializer_list<std::string_view> pieces) {
  size_t total_length = 0;
  for (std::string_view piece : pieces) total_length += piece.size();
  std::string text;
  text.reserve(total_length);
  for (std::string_view piece : pieces) text += piece;
  return text;
}

int get_physical_core_count() {
#ifdef __APPLE__
  int count = 0;
  size_t size = sizeof(count);
  if (sysctlbyname("hw.physicalcpu", &count, &size, nullptr, 0) == 0 &&
      count > 0) {
    return count;
  }
  return std::thread::hardware_concurrency();
#else
  cpu_set_t allowed;
  const bool masked = sched_getaffinity(0, sizeof(allowed), &allowed) == 0;
  auto trim = [](std::string s) {
    auto b = s.find_first_not_of(" \t");
    auto e = s.find_last_not_of(" \t");
    return (b == std::string::npos) ? "" : s.substr(b, e - b + 1);
  };
  // A core is a (package, core id) pair; core ids repeat across packages.
  std::ifstream cpuinfo("/proc/cpuinfo");
  std::string line;
  int processor = -1;
  int package = 0;
  std::set<std::pair<int, int> > cores;
  while (std::getline(cpuinfo, line)) {
    std::istringstream iss(line);
    std::string key, val;
    if (!std::getline(iss, key, ':') || !std::getline(iss, val)) continue;
    key = trim(key);
    if (key == "processor") {
      processor = std::stoi(trim(val));
      package = 0;
    } else if (key == "physical id") {
      package = std::stoi(trim(val));
    } else if (key == "core id" &&
               (!masked || (processor >= 0 && processor < CPU_SETSIZE &&
                            CPU_ISSET(processor, &allowed)))) {
      cores.emplace(package, std::stoi(trim(val)));
    }
  }
  if (!cores.empty()) return cores.size();
  if (masked) return CPU_COUNT(&allowed);
  return std::thread::hardware_concurrency();
#endif
}

}  // namespace tapa::internal
