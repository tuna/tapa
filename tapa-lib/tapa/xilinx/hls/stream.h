// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include "tapa/base/stream.h"

#include <cassert>
#include <cstddef>
#include <cstdint>
#include <type_traits>

#include <hls_fence.h>
#include <hls_stream.h>

#include "tapa/xilinx/hls/util.h"

namespace tapa {

// A clock boundary that HLS keeps every port access on its side of, while
// arithmetic may still cross it; in a pipelined loop it costs a stage, not
// initiation interval.
inline void fence() {
#pragma HLS inline
  ap_wait();
}

template <typename T>
class tapa_stream {
 public:
  tapa_stream() = default;
  tapa_stream(const char* name) {}
  tapa_stream(const tapa_stream&) = delete;

  bool empty() const {
#pragma HLS inline
    return _.empty();
  }

  bool try_eot(bool& is_eot) const {
#pragma HLS inline
    if (!empty()) {
      internal::elem_t<T> elem;
      _peek.read_nb(elem);
      is_eot = elem.eot;
      return true;
    }
    return false;
  }

  bool eot(bool& is_success) const {
#pragma HLS inline
    bool eot = false;
    is_success = try_eot(eot);
    return eot;
  }

  bool try_peek(T& value) const {
#pragma HLS inline
    if (!empty()) {
      internal::elem_t<T> elem;
      _peek.read_nb(elem);
      value = elem.val;
      return true;
    }
    return false;
  }

  T peek(bool& is_success) const {
#pragma HLS inline
    T val;
    is_success = try_peek(val);
    return val;
  }

  T peek(std::nullptr_t) const {
#pragma HLS inline
    T val{};
    try_peek(val);
    return val;
  }

  T peek(bool& is_success, bool& is_eot) const {
#pragma HLS inline
    internal::elem_t<T> peek_val;
    is_success = !empty() && _peek.read_nb(peek_val);
    if (is_success) {
      is_eot = peek_val.eot;
      return peek_val.val;
    }
    is_eot = false;
    return T{};
  }

  bool try_read(T& value) {
#pragma HLS inline
    internal::elem_t<T> elem;
    const bool is_success = _.read_nb(elem);
    value = elem.val;
    return is_success;
  }

  T read() {
#pragma HLS inline
    return _.read().val;
  }

  tapa_stream& operator>>(T& value) {
#pragma HLS inline
    value = read();
    return *this;
  }

  T read(bool& is_success) {
#pragma HLS inline
    internal::elem_t<T> elem;
    is_success = _.read_nb(elem);
    return elem.val;
  }

  T read(std::nullptr_t) {
#pragma HLS inline
    internal::elem_t<T> elem{};
    _.read_nb(elem);
    return elem.val;
  }

  template <typename U>
  void read(U* dst, int n) {
#pragma HLS inline
    internal::bulk_read<T>(*this, dst, n);
  }

  bool try_open() {
#pragma HLS inline
    internal::elem_t<T> elem;
    const bool succeeded = _.read_nb(elem);
    assert(!succeeded || elem.eot);
    return succeeded;
  }

  void open() {
#pragma HLS inline
    const auto elem = _.read();
    assert(elem.eot);
  }

  bool full() const {
#pragma HLS inline
    return _.full();
  }

  bool try_write(const T& value) {
#pragma HLS inline
    return _.write_nb({value, false});
  }

  void write(const T& value) {
#pragma HLS inline
    _.write({value, false});
  }

  tapa_stream& operator<<(const T& value) {
#pragma HLS inline
    write(value);
    return *this;
  }

  template <typename U>
  void write(const U* src, int n) {
#pragma HLS inline
    internal::bulk_write<T>(*this, src, n);
  }

  bool try_close() {
#pragma HLS inline
    internal::elem_t<T> elem;
    elem.val = {};
    elem.eot = true;
    return _.write_nb(elem);
  }

  void close() {
#pragma HLS inline
    internal::elem_t<T> elem;
    elem.eot = true;
    _.write(elem);
  }

  // The vendor's empty() is non-const even though it observes the queue.
  mutable hls::stream<internal::elem_t<T>> _;
  mutable hls::stream<internal::elem_t<T>> _peek;
};

namespace internal {

// The bits of a frame port's acquire token, which names the bank of the
// channel the frame occupies. A leaf is synthesized once whatever the depth
// of the streams it is given, so its bank port spans as many banks as a
// channel may have; the channel uses the address bits it needs.
inline constexpr int kFrameBankBits = 8;

}  // namespace internal

// A frame port. The channel hands out a bank with each acquire token and
// takes it back, in acquire order, with each release token; the task
// addresses element `i` of the frame at `bank * N + i`, so a frame acquired
// while the one before is still being released (as a pipelined loop does)
// addresses its own bank. tapacc puts `_buf` on an `ap_memory` port of one
// read (iframes) or write (oframes) port.
template <typename T, uint64_t N, bool kWrites>
class tapa_frames {
  using bank_t = ap_uint<internal::kFrameBankBits>;

  // What the views of both directions share: the bank they address and its
  // release.
  class frame {
   public:
    // The release waits for every access before it: HLS orders accesses to
    // different ports only by their data, and would otherwise hand the bank
    // back while an access to it is still to come.
    void release() {
#pragma HLS inline
      if (live_) {
        hls::fence({port_._buf}, {port_._release});
        port_._release.write(1);
        live_ = false;
      }
    }

    ~frame() {
#pragma HLS inline
      release();
    }

   protected:
    frame(tapa_frames& port, uint64_t base) : port_(port), base_(base) {}
    tapa_frames& port_;
    uint64_t base_;
    bool live_ = true;
  };

 public:
  class iview : public frame {
   public:
    T operator[](int i) const {
#pragma HLS inline
      return this->port_._buf[this->base_ + i];
    }

   private:
    friend class tapa_frames;
    using frame::frame;
  };

  class oview : public frame {
   public:
    // Element `i` of the frame; it can only be assigned.
    class slot {
     public:
      slot& operator=(const T& value) {
#pragma HLS inline
        port_._buf[at_] = value;
        return *this;
      }

     private:
      friend class oview;
      slot(tapa_frames& port, uint64_t at) : port_(port), at_(at) {}
      tapa_frames& port_;
      uint64_t at_;
    };

    slot operator[](int i) {
#pragma HLS inline
      return slot(this->port_, this->base_ + i);
    }

   private:
    friend class tapa_frames;
    using frame::frame;
  };

  using view = std::conditional_t<kWrites, oview, iview>;

  // The acquire polls, which keeps HLS from flattening or pipelining the
  // loop around it: a port's frames never overlap in time. A pipeline that
  // overlapped them would stall on the next acquire with the release of the
  // frame before still in a later stage, and a channel whose two ends both
  // did so would deadlock.
  view acquire() {
#pragma HLS inline
    bank_t bank;
    while (!_acquire.read_nb(bank)) {
    }
    return view(*this, uint64_t(bank) * N);
  }

  T _buf[N << internal::kFrameBankBits];
  hls::stream<bank_t> _acquire;
  hls::stream<ap_uint<1>> _release;
};

template <typename T, uint64_t N>
using iframes = tapa_frames<T, N, false>;

template <typename T, uint64_t N>
using oframes = tapa_frames<T, N, true>;

template <typename T>
using istream = tapa_stream<T>;

template <typename T>
using ostream = tapa_stream<T>;

template <typename T, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
using stream = tapa_stream<T>;

template <typename T, uint64_t S>
using istreams = istream<T>[S];

template <typename T, uint64_t S>
using ostreams = ostream<T>[S];

template <typename T, uint64_t S, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
using streams = stream<T, N>[S];

}  // namespace tapa
