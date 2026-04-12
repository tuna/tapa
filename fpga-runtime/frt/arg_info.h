// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_ARG_INFO_H_
#define FPGA_RUNTIME_ARG_INFO_H_

#include <ostream>
#include <string>

namespace fpga {

struct ArgInfo {
  enum Cat {
    kScalar = 0,
    kMmap = 1,
    kStream = 2,
    kStreams = 3,
  };
  int index = 0;
  std::string name;
  std::string type;
  Cat cat = kScalar;
};

inline std::ostream& operator<<(std::ostream& os, const ArgInfo::Cat& cat) {
  switch (cat) {
    case ArgInfo::kScalar:
      return os << "scalar";
    case ArgInfo::kMmap:
      return os << "mmap";
    case ArgInfo::kStream:
      return os << "stream";
    case ArgInfo::kStreams:
      return os << "streams";
  }
  return os << "unknown";
}

inline std::ostream& operator<<(std::ostream& os, const ArgInfo& arg) {
  return os << "ArgInfo(index=" << arg.index << ", name=" << arg.name
            << ", type=" << arg.type << ", cat=" << arg.cat << ")";
}

}  // namespace fpga

#endif  // FPGA_RUNTIME_ARG_INFO_H_
