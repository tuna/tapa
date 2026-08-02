// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include "tapa/base/stream.h"

#include <cassert>
#include <cstddef>
#include <cstdint>

#include <hls_stream.h>

namespace tapa {

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
    T val;
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
    internal::elem_t<T> elem;
    _.read_nb(elem);
    return elem.val;
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

  hls::stream<internal::elem_t<T>> _;
  mutable hls::stream<internal::elem_t<T>> _peek;
};

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
