// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <climits>
#include <cstdint>
#include <ostream>
#include <type_traits>

namespace tapa {

namespace internal {

template <typename T, int width = T::width>
inline constexpr int widthof(int) {
  return T::width;
}

template <typename T>
inline constexpr int widthof(short) {
  return sizeof(T) * CHAR_BIT;
}

}  // namespace internal

template <typename T>
inline constexpr int widthof() {
  return internal::widthof<T>(0);
}

template <typename T>
inline constexpr int widthof(T object) {
  return internal::widthof<T>(0);
}

template <uint64_t N>
inline constexpr uint64_t round_up_div(uint64_t i) {
  return i == 0 ? 0 : ((i - 1) / N + 1);
}

template <uint64_t N>
inline constexpr uint64_t round_up(uint64_t i) {
  return i == 0 ? 0 : ((i - 1) / N + 1) * N;
}

/// Yields one clock cycle on synthesis targets (vendor ap_wait). A no-op in
/// software simulation, where tasks run as coroutines without a clock.
void wait() noexcept;

/// Yields @p n clock cycles on synthesis targets (vendor ap_wait_n). A no-op
/// in software simulation, like the nullary form.
void wait(int n) noexcept;

template <typename To, typename From>
inline typename std::enable_if<sizeof(To) == sizeof(From), To>::type  //
bit_cast(From from) noexcept;

template <typename Addr, typename Payload>
struct packet {
  Addr addr;
  Payload payload;
};

template <typename Addr, typename Payload>
inline std::ostream& operator<<(std::ostream& os,
                                const packet<Addr, Payload>& obj) {
  return os << "{addr: " << obj.addr << ", payload: " << obj.payload << "}";
}

}  // namespace tapa

#define TAPA_WHILE_NOT_EOT(fifo)                                \
  for (bool tapa_##fifo##_valid;                                \
       !fifo.eot(tapa_##fifo##_valid) || !tapa_##fifo##_valid;) \
    if (tapa_##fifo##_valid)

#define TAPA_WHILE_NEITHER_EOT(fifo1, fifo2)                          \
  for (bool tapa_##fifo1##_valid, tapa_##fifo2##_valid;               \
       (!fifo1.eot(tapa_##fifo1##_valid) || !tapa_##fifo1##_valid) && \
       (!fifo2.eot(tapa_##fifo2##_valid) || !tapa_##fifo2##_valid);)  \
    if (tapa_##fifo1##_valid && tapa_##fifo2##_valid)

#define TAPA_WHILE_NONE_EOT(fifo1, fifo2, fifo3)                              \
  for (bool tapa_##fifo1##_valid, tapa_##fifo2##_valid, tapa_##fifo3##_valid; \
       (!fifo1.eot(tapa_##fifo1##_valid) || !tapa_##fifo1##_valid) &&         \
       (!fifo2.eot(tapa_##fifo2##_valid) || !tapa_##fifo2##_valid) &&         \
       (!fifo3.eot(tapa_##fifo3##_valid) || !tapa_##fifo3##_valid);)          \
    if (tapa_##fifo1##_valid && tapa_##fifo2##_valid && tapa_##fifo3##_valid)
