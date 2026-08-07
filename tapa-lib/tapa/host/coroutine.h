// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <functional>
#include <string>

namespace tapa {
namespace internal {
void schedule(bool detach, const std::function<void()>&);
void schedule_cleanup(const std::function<void()>&);
void yield(const std::string& msg);

// FRT instance lifecycle, observed by blocked stream operations. A stream
// bound to a kernel instance can only be filled or drained by that
// instance, so once every scheduled instance has finished, a blocked
// operation on such a stream can never make progress.
void note_frt_instance_scheduled();
void note_frt_instance_finished();
bool every_frt_instance_finished();
}  // namespace internal
}  // namespace tapa
