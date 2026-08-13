// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "DILATE.h"

void unikernel(
    tapa::mmap<INTERFACE_WIDTH> in_0, tapa::mmap<INTERFACE_WIDTH> out_0,
    tapa::mmap<INTERFACE_WIDTH> in_1, tapa::mmap<INTERFACE_WIDTH> out_1,
    tapa::mmap<INTERFACE_WIDTH> in_2, tapa::mmap<INTERFACE_WIDTH> out_2,
    tapa::mmap<INTERFACE_WIDTH> in_3, tapa::mmap<INTERFACE_WIDTH> out_3,
    tapa::mmap<INTERFACE_WIDTH> in_4, tapa::mmap<INTERFACE_WIDTH> out_4,
    tapa::mmap<INTERFACE_WIDTH> in_5, tapa::mmap<INTERFACE_WIDTH> out_5,
    tapa::mmap<INTERFACE_WIDTH> in_6, tapa::mmap<INTERFACE_WIDTH> out_6,
    tapa::mmap<INTERFACE_WIDTH> in_7, tapa::mmap<INTERFACE_WIDTH> out_7,
    tapa::mmap<INTERFACE_WIDTH> in_8, tapa::mmap<INTERFACE_WIDTH> out_8,
    tapa::mmap<INTERFACE_WIDTH> in_9, tapa::mmap<INTERFACE_WIDTH> out_9,
    tapa::mmap<INTERFACE_WIDTH> in_10, tapa::mmap<INTERFACE_WIDTH> out_10,
    tapa::mmap<INTERFACE_WIDTH> in_11, tapa::mmap<INTERFACE_WIDTH> out_11,
    tapa::mmap<INTERFACE_WIDTH> in_12, tapa::mmap<INTERFACE_WIDTH> out_12,
    tapa::mmap<INTERFACE_WIDTH> in_13, tapa::mmap<INTERFACE_WIDTH> out_13,
    tapa::mmap<INTERFACE_WIDTH> in_14, tapa::mmap<INTERFACE_WIDTH> out_14,
    uint32_t iters);

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

template <typename T>
using AlignedVector = std::vector<T, tapa::aligned_allocator<T>>;

namespace {

constexpr uint32_t kIterations = 1;
constexpr int kPayloadBeats = GRID_COLS / WIDTH_FACTOR * PART_ROWS;
constexpr int kBufferBeats = kPayloadBeats + kDilateWindowBeats;

uint32_t FloatBits(float value) {
  uint32_t bits;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

float BitsFloat(uint32_t bits) {
  float value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}

float ReadFloat(const AlignedVector<INTERFACE_WIDTH>& data, size_t index) {
  const size_t word = index / WIDTH_FACTOR;
  const size_t lane = index % WIDTH_FACTOR;
  return BitsFloat(data[word].range(lane * 32 + 31, lane * 32));
}

float Expected(const AlignedVector<INTERFACE_WIDTH>& input, size_t index) {
  constexpr std::array<int, 13> kOffsets = {
      2,
      GRID_COLS + 1,
      GRID_COLS + 2,
      GRID_COLS + 3,
      2 * GRID_COLS,
      2 * GRID_COLS + 1,
      2 * GRID_COLS + 2,
      2 * GRID_COLS + 3,
      2 * GRID_COLS + 4,
      3 * GRID_COLS + 1,
      3 * GRID_COLS + 2,
      3 * GRID_COLS + 3,
      4 * GRID_COLS + 2,
  };
  float expected = ReadFloat(input, index + kOffsets.front());
  for (const int offset : kOffsets) {
    expected = std::max(expected, ReadFloat(input, index + offset));
  }
  return expected;
}

}  // namespace

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  std::array<AlignedVector<INTERFACE_WIDTH>, KERNEL_COUNT> inputs;
  std::array<AlignedVector<INTERFACE_WIDTH>, KERNEL_COUNT> outputs;
  for (int channel = 0; channel < KERNEL_COUNT; ++channel) {
    inputs[channel].resize(kBufferBeats);
    outputs[channel].resize(kBufferBeats);
    for (int word = 0; word < kBufferBeats; ++word) {
      INTERFACE_WIDTH packed = 0;
      for (int lane = 0; lane < WIDTH_FACTOR; ++lane) {
        const size_t index = static_cast<size_t>(word) * WIDTH_FACTOR + lane;
        const float value =
            static_cast<float>((channel + 1) * 1024 + (index * 17) % 997);
        packed.range(lane * 32 + 31, lane * 32) = FloatBits(value);
      }
      inputs[channel][word] = packed;
    }
  }

  std::clog << "running " << KERNEL_COUNT << " dilation channels over "
            << kPayloadBeats << " packed words\n";
  const int64_t kernel_time_ns = tapa::invoke(
      unikernel, FLAGS_bitstream,
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[0]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[0]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[1]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[1]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[2]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[2]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[3]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[3]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[4]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[4]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[5]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[5]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[6]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[6]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[7]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[7]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[8]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[8]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[9]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[9]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[10]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[10]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[11]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[11]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[12]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[12]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[13]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[13]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(inputs[14]),
      tapa::read_write_mmap<INTERFACE_WIDTH>(outputs[14]), kIterations);
  std::clog << "kernel time: " << kernel_time_ns << " ns\n";

  uint64_t errors = 0;
  constexpr uint64_t kMaxReportedErrors = 10;
  for (int channel = 0; channel < KERNEL_COUNT; ++channel) {
    for (size_t index = 0; index < size_t{kPayloadBeats} * WIDTH_FACTOR;
         ++index) {
      const float expected = Expected(inputs[channel], index);
      const float actual = ReadFloat(
          outputs[channel], size_t{kDilateWindowBeats} * WIDTH_FACTOR + index);
      if (actual != expected) {
        if (errors < kMaxReportedErrors) {
          std::clog << "channel " << channel << ", element " << index
                    << ": expected " << expected << ", got " << actual << '\n';
        }
        ++errors;
      }
    }
  }

  if (errors != 0) {
    std::clog << "FAIL: " << errors << " mismatches\n";
    return EXIT_FAILURE;
  }
  std::clog << "PASS\n";
  return EXIT_SUCCESS;
}
