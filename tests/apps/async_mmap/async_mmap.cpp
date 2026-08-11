// Demonstrates the `tapa::async_mmap` port category: a leaf task drives the
// read-address and read-data channels independently, streams the values on,
// and a second leaf writes them back through a plain `tapa::mmap`.
//
// Also the `async_mmap` entry in the examples catalog, so it has to survive
// `tapa compile`. Two constraints follow from that, and both are easy to trip
// over when editing this file:
//
//   * Port names avoid the Verilog / Vitis HLS reserved words `tapa synth`
//     rejects (`in`, `out`, `reg`, `wire`, ...).
//   * A parameter that carries a channel is `tapa::istream` or
//     `tapa::ostream`, never `tapa::stream` — that one declares a channel and
//     belongs in an upper task's body, as `data_q` does below.

#include "async_mmap.h"

void AsyncReader(tapa::async_mmap<float>& mem, uint64_t n,
                 tapa::ostream<float>& data_q) {
  // Requests and responses advance independently: that decoupling is the
  // whole point of `async_mmap`.
  for (uint64_t i_req = 0, i_resp = 0; i_resp < n;) {
#pragma HLS pipeline II = 1
    if (i_req < n && !mem.read_addr.full()) {
      mem.read_addr.try_write(i_req);
      ++i_req;
    }
    if (!data_q.full() && !mem.read_data.empty()) {
      float value;
      mem.read_data.try_read(value);
      data_q.try_write(value);
      ++i_resp;
    }
  }
}

void Writer(tapa::istream<float>& data_q, tapa::mmap<float> dst, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
#pragma HLS pipeline II = 1
    dst[i] = data_q.read();
  }
}

void AsyncTop(tapa::mmap<float> mem, tapa::mmap<float> dst, uint64_t n) {
  tapa::stream<float> data_q("data_q");

  tapa::task()
      .invoke(AsyncReader, mem, n, data_q)
      .invoke(Writer, data_q, dst, n);
}
