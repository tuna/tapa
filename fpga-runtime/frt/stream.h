// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_STREAM_H_
#define FPGA_RUNTIME_STREAM_H_

#include <deque>
#include <mutex>

#include <glog/logging.h>

#include "frt/stream_arg.h"
#include "frt/stringify.h"
#include "frt/tag.h"

namespace fpga {
namespace internal {

template <typename T, Tag tag>
class Stream;

template <typename T>
class Stream<T, Tag::kReadWrite> : public StreamArg {
 public:
  explicit Stream(uint64_t depth)
      : StreamArg(nullptr), depth_(depth == 0 ? 1 : depth) {}

  bool empty() const {
    std::lock_guard<std::mutex> lock(mu_);
    return queue_.empty();
  }

  bool full() const {
    std::lock_guard<std::mutex> lock(mu_);
    return queue_.size() >= depth_;
  }

  void push(const T& val) {
    std::lock_guard<std::mutex> lock(mu_);
    CHECK_LT(queue_.size(), depth_) << "stream is full";
    queue_.push_back(val);
  }

  T pop() {
    std::lock_guard<std::mutex> lock(mu_);
    CHECK(!queue_.empty()) << "stream is empty";
    T val = queue_.front();
    queue_.pop_front();
    return val;
  }

  T front() const {
    std::lock_guard<std::mutex> lock(mu_);
    CHECK(!queue_.empty()) << "stream is empty";
    return queue_.front();
  }

 private:
  const size_t depth_;
  mutable std::mutex mu_;
  std::deque<T> queue_;
};

}  // namespace internal
}  // namespace fpga

#endif  // FPGA_RUNTIME_STREAM_H_
