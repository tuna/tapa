// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_TESTS_REGRESSION_SPARSE_FIXTURE_H_
#define TAPA_TESTS_REGRESSION_SPARSE_FIXTURE_H_

#include <cstddef>
#include <vector>

namespace tapa::regression {

// Fixed sparse architectures read a compile-time number of memory beats. The
// final edge pointer must cover the same dummy beats so downstream PEs consume
// everything the memory tasks produce; resizing only the backing buffers
// deadlocks the stream graph.
template <typename Edge>
bool PadSparseEdgeLists(std::vector<std::vector<Edge>>& edge_lists,
                        std::vector<int>& edge_list_ptr,
                        std::size_t beats_per_channel) {
  if (edge_list_ptr.empty() || edge_list_ptr.back() < 0 ||
      static_cast<std::size_t>(edge_list_ptr.back()) > beats_per_channel) {
    return false;
  }
  for (auto& edge_list : edge_lists) {
    edge_list.resize(beats_per_channel, Edge{-1, -1, 0});
  }
  edge_list_ptr.back() = static_cast<int>(beats_per_channel);
  return true;
}

}  // namespace tapa::regression

#endif  // TAPA_TESTS_REGRESSION_SPARSE_FIXTURE_H_
