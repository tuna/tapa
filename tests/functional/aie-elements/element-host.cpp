#include <iostream>
#include <vector>

#include <gflags/gflags.h>

#include "element.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const int n = argc > 1 ? atoi(argv[1]) : 5;
  std::vector<Pair> pair_a(n), pair_b(n), pair_c(n);
  std::vector<Vec3> vec_a(n), vec_b(n), vec_c(n);
  for (int i = 0; i < n; ++i) {
    pair_a[i] = {uint64_t(i), uint64_t(2 * i)};
    pair_b[i] = {uint64_t(3 * i), uint64_t(4 * i)};
    for (int lane = 0; lane < 3; ++lane) {
      vec_a[i].set(lane, float(i + lane));
      vec_b[i].set(lane, float(2 * (i + lane)));
    }
  }
  tapa::invoke(
      Elements, FLAGS_bitstream, tapa::read_only_mmap<Pair>(pair_a),
      tapa::read_only_mmap<Pair>(pair_b), tapa::write_only_mmap<Pair>(pair_c),
      tapa::read_only_mmap<Vec3>(vec_a), tapa::read_only_mmap<Vec3>(vec_b),
      tapa::write_only_mmap<Vec3>(vec_c), n);

  int errors = 0;
  for (int i = 0; i < n; ++i) {
    if (pair_c[i].x != uint64_t(4 * i) || pair_c[i].y != uint64_t(6 * i) ||
        vec_c[i].get(0) != vec_a[i].get(0) + vec_b[i].get(0) ||
        vec_c[i].get(1) != vec_a[i].get(1) + vec_b[i].get(1) ||
        vec_c[i].get(2) != vec_a[i].get(2) + vec_b[i].get(2)) {
      ++errors;
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors != 0;
}
