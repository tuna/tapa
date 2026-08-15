// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstdint>
#include <functional>
#include <string>

namespace tapa {
namespace internal {
void schedule(bool detach, const std::function<void()>&);
void schedule_cleanup(const std::function<void()>&);

// Yield to the scheduler, naming what the caller is blocked on.
//
// The reason is formatted only when stream debugging is on, and the
// non-coroutine build ignores it outright — but a blocked producer/consumer
// pair yields once per element, so callers must not build a message eagerly.
// Both overloads take pieces the caller already holds (`get_name()` returns a
// reference) so a yield costs no allocation.
void yield(const char* reason);
void yield(const std::string& channel_name, const char* state);

// How many times in a row this thread has found itself unable to proceed.
//
// A thread that cannot make progress backs off progressively, from handing
// the CPU over to sleeping, so a genuinely idle thread costs no spin. But a
// producer/consumer pair alternates between blocking and making progress, and
// with no reset the streak would climb until every poll slept a millisecond
// again — which is the whole cost of a simulation without coroutines. Any
// successful poll clears it.
extern thread_local int blocked_poll_streak;

// How many blocked polls this thread has made since a poll last succeeded.
//
// Distinct from `blocked_poll_streak`: the streak drives the idle backoff,
// so the coroutine scheduler resets it after every round that ran a
// coroutine. This count feeds the stall detector and must survive those
// rounds — a blocked coroutine still runs every round — so only a
// successful poll zeroes it. A thread that keeps getting served never
// reaches the first clock read.
extern thread_local uint64_t blocked_poll_count;

inline void note_poll_progress() {
  blocked_poll_streak = 0;
  blocked_poll_count = 0;
}

// Warn once when a channel stays blocked past the stall threshold.
//
// Called only from the blocked path, so the clock reads land where the
// simulation is already waiting and the success path stays free.
void note_blocked_poll(const std::string& channel_name, const char* state);

// Seconds parsed out of `TAPA_STALL_WARN_SECONDS`, as nanoseconds; 0 disables
// the warning. Exposed for testing; `text` may be null or empty for "unset".
uint64_t parse_stall_warn_seconds(const char* text);

// FRT instance lifecycle, observed by blocked stream operations. A stream
// bound to a kernel instance can only be filled or drained by that
// instance, so once every scheduled instance has finished, a blocked
// operation on such a stream can never make progress.
void note_frt_instance_scheduled();
void note_frt_instance_finished();
bool every_frt_instance_finished();
}  // namespace internal
}  // namespace tapa
