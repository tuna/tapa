// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#include "frt/arg_info.h"

namespace fpga {

std::ostream& operator<<(std::ostream& os, const ArgInfo::Cat& cat) {
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

std::ostream& operator<<(std::ostream& os, const ArgInfo& arg) {
  return os << "ArgInfo(index=" << arg.index << ", name=" << arg.name
            << ", type=" << arg.type << ", cat=" << arg.cat << ")";
}

}  // namespace fpga
