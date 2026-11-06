#pragma once

#include <array>
#include <cstdint>

#include "tapa/base/mmap.h"
#include "tapa/base/util.h"

// The PL movers that serve an AI Engine task's async_mmap. TAPA instantiates
// one per direction a port uses, bound to the memory the port is bound to;
// the AI Engine task's requests and data reach them over PLIO. The same code
// runs in software when the AI Engine side is simulated.
#if defined(TAPA_TARGET_XILINX_HLS_)
#define TAPA_MOVER_PIPELINE _Pragma("HLS pipeline II = 1")
#else
#define TAPA_MOVER_PIPELINE
#endif

namespace tapa {
namespace internal {

// Forwards each read request to `mem` and its data back, in request order,
// until the end request, then returns once the data already requested has
// gone out.
template <typename T>
void serve_reads(tapa::async_mmap<T>& mem, tapa::istream<burst>& requests,
                 tapa::ostream<T>& data) {
  // Two counters, not one difference, so neither update waits on the other.
  int64_t requested = 0;  // elements requested
  int64_t sent = 0;       // elements sent back
  bool ending = false;
  for (;;) {
    TAPA_MOVER_PIPELINE
    // Decided on the last iteration's state, off this iteration's path.
    if (ending && sent == requested) break;
    burst request;
    if (!ending && !mem.read_addr.full() && requests.try_read(request)) {
      if (ends_requests(request)) {
        ending = true;
      } else {
        mem.read_addr.write(request);
        requested += request.n;
      }
    }
    T value;
    if (!data.full() && mem.read_data.try_read(value)) {
      data.write(value);
      ++sent;
    }
  }
}

// The write responses of a body that never reads them.
struct no_responses {
  bool full() const { return false; }
  void write(uint32_t) {}
};

// Forwards each write request, read from `requests` with its data right
// behind it (one channel, since a kernel has two output stream ports), and
// every response, until the end request; then returns once every element
// written is acknowledged. A request is the first sizeof(burst) bytes of
// the elements it occupies.
template <typename T, typename Responses>
void forward_writes(tapa::async_mmap<T>& mem, tapa::istream<T>& requests,
                    Responses& responses) {
  static_assert(sizeof(burst) % sizeof(T) == 0,
                "an AI Engine memory element is 4, 8 or 16 bytes");
  constexpr int kParts = sizeof(burst) / sizeof(T);
  std::array<T, kParts> parts;
  int part = 0;           // elements of the request read so far
  int64_t left = 0;       // elements of data still to come for it
  int64_t requested = 0;  // elements requested
  int64_t acked = 0;      // elements the memory acknowledged
  bool ending = false;
  for (;;) {
    TAPA_MOVER_PIPELINE
    // Decided on the last iteration's state, off this iteration's path.
    if (ending && acked == requested) break;
    T value;
    if (!ending && !mem.write_addr.full() && !mem.write_data.full() &&
        requests.try_read(value)) {
      if (left > 0) {
        mem.write_data.write(value);
        --left;
      } else {
        parts[part] = value;
        if (++part == kParts) {
          part = 0;
          const burst request = bit_cast<std::array<burst, 1>>(parts)[0];
          if (ends_requests(request)) {
            ending = true;
          } else {
            mem.write_addr.write(request);
            left = request.n;
            requested += request.n;
          }
        }
      }
    }
    uint8_t ack;
    if (!responses.full() && mem.write_resp.try_read(ack)) {
      responses.write(ack);
      acked += int64_t{ack} + 1;
    }
  }
}

template <typename T>
void serve_writes(tapa::async_mmap<T>& mem, tapa::istream<T>& requests,
                  tapa::ostream<uint32_t>& responses) {
  forward_writes(mem, requests, responses);
}

template <typename T>
void serve_writes(tapa::async_mmap<T>& mem, tapa::istream<T>& requests) {
  no_responses responses;
  forward_writes(mem, requests, responses);
}

}  // namespace internal
}  // namespace tapa
