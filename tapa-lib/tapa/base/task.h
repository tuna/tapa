// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_BASE_TASK_H_
#define TAPA_BASE_TASK_H_

namespace tapa {

namespace internal {
enum class InvokeMode {
  kJoin = 0,
  kDetach = -1,
  kSequential = 1,
};
}  // namespace internal

inline constexpr auto join = internal::InvokeMode::kJoin;
inline constexpr auto detach = internal::InvokeMode::kDetach;

struct seq {
  seq() = default;
  seq(const seq&) = delete;
  seq(seq&&) = delete;
  seq& operator=(const seq&) = delete;
  seq& operator=(seq&&) = delete;
  int pos = 0;
};

}  // namespace tapa

#endif  // TAPA_BASE_TASK_H_
