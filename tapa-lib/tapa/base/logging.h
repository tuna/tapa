// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

namespace tapa {
namespace internal {

struct dummy {
  template <typename T>
  dummy& operator<<(const T&) {
    return *this;
  }
};

}  // namespace internal
}  // namespace tapa
