// Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstdlib>
#include <optional>
#include <string>
#include <string_view>

#include <glog/logging.h>

namespace tapa_testing {

class ScopedSetEnv {
 public:
  explicit ScopedSetEnv(std::string_view name, const char* value)
      : name_(name) {
    if (const char* old_value = getenv(name_.c_str()); old_value != nullptr) {
      old_value_ = strdup(old_value);
    }
    SetValue(value);
  }

  ScopedSetEnv(const ScopedSetEnv&) = delete;
  ScopedSetEnv& operator=(const ScopedSetEnv&) = delete;

  ~ScopedSetEnv() { SetValue(old_value_ ? old_value_->c_str() : nullptr); }

 private:
  void SetValue(const char* value) {
    if (value == nullptr) {
      PLOG_IF(ERROR, unsetenv(name_.c_str())) << "unsetenv";
    } else {
      PLOG_IF(ERROR, setenv(name_.c_str(), value, /*replace=*/1)) << "setenv";
    }
  }
  const std::string name_;
  std::optional<std::string> old_value_;
};

}  // namespace tapa_testing
