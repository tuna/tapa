// Test that software simulation works on macOS via tapa-sim.

#include "tapa/host/tapa.h"

#include <cstdint>
#include <vector>

#include <gtest/gtest.h>

void Add(tapa::istream<int>& in, tapa::ostream<int>& out) {
  int val = in.read();
  out.write(val + 1);
}

void Top(tapa::mmap<int> data, int n) {
  tapa::stream<int> in("in");
  tapa::stream<int> out("out");

  // Write input data to stream
  for (int i = 0; i < n; ++i) {
    in.write(data[i]);
  }

  // Process
  tapa::task().invoke(Add, in, out);

  // Read output
  data[0] = out.read();
}

TEST(TapaSimTest, BasicStreamSimulation) {
  std::vector<int> data = {41};
  Top(tapa::mmap<int>(data.data(), data.size()), 1);
  EXPECT_EQ(data[0], 42);
}

TEST(TapaSimTest, InvokeWithEmptyBitstream) {
  std::vector<int> data = {99};
  auto ns =
      tapa::invoke(Top, /*bitstream=*/"",
                   tapa::read_write_mmap<int>(data.data(), data.size()), 1);
  EXPECT_EQ(data[0], 100);
  EXPECT_GT(ns, 0);
}
