#include <cmath>
#include <complex>
#include <cstdlib>
#include <iostream>
#include <random>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "fft2d.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

namespace {

using complex = std::complex<double>;

int BitReverse(int i) {
  int r = 0;
  for (int b = 0; b < 5; ++b) r |= ((i >> b) & 1) << (4 - b);
  return r;
}

}  // namespace

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const int n = argc > 1 ? atoi(argv[1]) : 4;

  std::vector<vec, tapa::aligned_allocator<vec>> src(n * kFrame);
  std::vector<vec, tapa::aligned_allocator<vec>> dst(n * kFrame);
  std::vector<complex> x(n * kEdge * kEdge);
  std::mt19937 gen(7);
  std::uniform_real_distribution<float> dist(-1.f, 1.f);
  for (int f = 0; f < n; ++f) {
    for (int r = 0; r < kEdge; ++r) {
      for (int c = 0; c < kEdge; ++c) {
        const float re = dist(gen), im = dist(gen);
        x[(f * kEdge + r) * kEdge + c] = {re, im};
        vec* row = &src[f * kFrame + r * 2 * kGroups];
        row[c / kLanes].set(c % kLanes, re);
        row[kGroups + c / kLanes].set(c % kLanes, im);
      }
    }
  }
  const int64_t ns =
      tapa::invoke(Fft2d, FLAGS_bitstream, tapa::read_only_mmap<const vec>(src),
                   tapa::write_only_mmap<vec>(dst), n);
  std::clog << "kernel time: " << ns * 1e-9 << " s" << std::endl;

  // Output position (h, p, lane) holds frequency (k1, k2) = (bitrev(p),
  // bitrev(8h + lane)).
  double worst = 0;
  for (int f = 0; f < n; ++f) {
    for (int h = 0; h < kGroups; ++h) {
      for (int p = 0; p < kEdge; ++p) {
        for (int lane = 0; lane < kLanes; ++lane) {
          const int k1 = BitReverse(p), k2 = BitReverse(h * kLanes + lane);
          complex expected = 0;
          for (int r = 0; r < kEdge; ++r) {
            for (int c = 0; c < kEdge; ++c) {
              expected +=
                  x[(f * kEdge + r) * kEdge + c] *
                  std::polar(1.0, -2 * M_PI * (k1 * r + k2 * c) / kEdge);
            }
          }
          const vec* out = &dst[f * kFrame + (h * kEdge + p) * 2];
          const complex actual = {out[0][lane], out[1][lane]};
          worst = std::max(worst, std::abs(actual - expected));
        }
      }
    }
  }
  // Each output sums 1024 inputs of magnitude below sqrt(2).
  const bool pass = worst < 1e-3;
  std::clog << "largest error " << worst << (pass ? ": PASS!" : ": FAIL!")
            << std::endl;
  return pass ? 0 : 1;
}
