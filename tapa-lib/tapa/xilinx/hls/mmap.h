// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstdint>

#include <ap_int.h>

#include "tapa/base/mmap.h"
#include "tapa/xilinx/hls/stream.h"

namespace tapa {
namespace internal {

// An address channel of an async_mmap: each request is `{count, byte
// address}`, which the bridge expands into one element address per cycle.
template <typename addr_t, typename T>
class addr_ostream {
 public:
  using req_t = ap_uint<96>;

  bool full() const {
#pragma HLS inline
    return _.full();
  }

  bool try_write(const addr_t& value) {
#pragma HLS inline
    return _.write_nb(request(value, 1));
  }

  bool try_write(const burst& value) {
#pragma HLS inline
    return _.write_nb(request(value.offset, value.n));
  }

  void write(const addr_t& value) {
#pragma HLS inline
    _.write(request(value, 1));
  }

  void write(const burst& value) {
#pragma HLS inline
    _.write(request(value.offset, value.n));
  }

  addr_ostream& operator<<(const addr_t& value) {
#pragma HLS inline
    write(value);
    return *this;
  }

  addr_ostream& operator<<(const burst& value) {
#pragma HLS inline
    write(value);
    return *this;
  }

  bool try_close() { return true; }

  void close() {}

  hls::stream<req_t> _;
  const addr_t offset;

 private:
  req_t request(addr_t index, int64_t n) const {
#pragma HLS inline
    req_t req;
    req.range(63, 0) = offset + index * sizeof(T);
    req.range(95, 64) = n;
    return req;
  }
};

}  // namespace internal

template <typename T>
using mmap = T*;

template <typename T>
struct async_mmap {
  using addr_t = int64_t;
  using resp_t = uint8_t;

  tapa::internal::addr_ostream<addr_t, T> read_addr;
  tapa::istream<T> read_data;
  tapa::internal::addr_ostream<addr_t, T> write_addr;
  tapa::ostream<T> write_data;
  tapa::istream<resp_t> write_resp;
};

template <typename T, uint64_t S>
class mmaps;

template <typename T, int chan_count, int64_t chan_size>
class hmap;

}  // namespace tapa
