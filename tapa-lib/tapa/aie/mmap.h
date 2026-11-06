#pragma once

#include <cstdint>

#include <adf.h>

#include "tapa/aie/stream.h"
#include "tapa/base/mmap.h"

// An AI Engine task's async_mmap, served from the PL by movers (see
// `tapa/base/mover.h`) over ADF stream ports: reads take a request port and a
// data port; writes take one port, a kernel having only two output stream
// ports, so a write request's data follows it on that port before the next
// write request, plus a response port when the body reads `write_resp`. As on
// AI Engine streams, every operation blocks: `full()` is always false and
// `try_*` report success once the transfer completes, so a body requests
// ahead of the data it then reads instead of polling.

namespace tapa {
namespace internal {
namespace aie {

// A request channel; its port is null when the body never names it.
class request_ostream {
 public:
  explicit request_ostream(output_stream<int32>* port) : port_(port) {}

  bool full() const { return false; }
  TAPA_AIE_INLINE void write(const burst& value) {
    elem_io<burst>::write(port_, value);
    after_write();
  }
  TAPA_AIE_INLINE void write(int64_t index) { write(burst(index, 1)); }
  TAPA_AIE_INLINE bool try_write(const burst& value) {
    write(value);
    return true;
  }
  TAPA_AIE_INLINE bool try_write(int64_t index) {
    write(index);
    return true;
  }
  TAPA_AIE_INLINE request_ostream& operator<<(const burst& value) {
    write(value);
    return *this;
  }
  TAPA_AIE_INLINE request_ostream& operator<<(int64_t index) {
    write(index);
    return *this;
  }

  // Tells the mover the body has returned.
  void end() {
    if (port_ != nullptr) write(end_of_requests());
  }

 private:
  output_stream<int32>* port_;
};

// The write response channel: each 32-bit word is one acknowledgement.
class response_istream {
 public:
  explicit response_istream(input_stream<int32>* port) : port_(port) {}

  TAPA_AIE_INLINE uint8_t read() { return uint8_t(readincr(port_)); }
  TAPA_AIE_INLINE bool try_read(uint8_t& value) {
    value = read();
    return true;
  }
  TAPA_AIE_INLINE response_istream& operator>>(uint8_t& value) {
    value = read();
    return *this;
  }

 private:
  input_stream<int32>* port_;
};

}  // namespace aie
}  // namespace internal

namespace aie {

// The ADF ports of one async_mmap, as the generated kernel wrapper passes
// them; null for a channel the body never names.
struct memory {
  output_stream<int32>* read_addr;
  input_stream<int32>* read_data;
  output_stream<int32>* write_addr;
  output_stream<int32>* write_data;
  input_stream<int32>* write_resp;
};

}  // namespace aie

template <typename T>
struct async_mmap {
  using addr_t = int64_t;
  using resp_t = uint8_t;

  explicit async_mmap(const aie::memory& ports)
      : read_addr(ports.read_addr),
        read_data(ports.read_data),
        write_addr(ports.write_addr),
        write_data(ports.write_data),
        write_resp(ports.write_resp) {}

  internal::aie::request_ostream read_addr;
  istream<T> read_data;
  internal::aie::request_ostream write_addr;
  ostream<T> write_data;
  internal::aie::response_istream write_resp;
};

namespace aie {

template <typename T>
struct arg<tapa::async_mmap<T>&> {
  tapa::async_mmap<T> value;
  explicit arg(const memory& ports) : value(ports) {}
  // The holder is a temporary of the invoke's full expression, so the end
  // requests go out once the task has returned.
  ~arg() {
    value.read_addr.end();
    value.write_addr.end();
  }
  TAPA_AIE_INLINE tapa::async_mmap<T>& get() { return value; }
};

}  // namespace aie
}  // namespace tapa
