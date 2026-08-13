// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tests/regression/sparse_fixture.h"

#include <gtest/gtest.h>

namespace {

struct Edge {
  int col;
  int row;
  float value;
};

TEST(SparseFixtureTest, PadsControlPointerAndEveryMemoryChannel) {
  std::vector<std::vector<Edge>> edge_lists = {
      {{0, 0, 1.0F}},
      {{0, 0, 1.0F}, {1, 1, 1.0F}},
  };
  std::vector<int> edge_list_ptr = {0, 2};

  ASSERT_TRUE(tapa::regression::PadSparseEdgeLists(edge_lists, edge_list_ptr,
                                                   /*beats_per_channel=*/4));
  EXPECT_EQ(edge_list_ptr.back(), 4);
  ASSERT_EQ(edge_lists[0].size(), 4);
  ASSERT_EQ(edge_lists[1].size(), 4);
  EXPECT_EQ(edge_lists[0][3].row, -1);
  EXPECT_EQ(edge_lists[1][3].row, -1);
}

TEST(SparseFixtureTest, RejectsInputLargerThanFixedArchitecture) {
  std::vector<std::vector<Edge>> edge_lists = {{{0, 0, 1.0F}}};
  std::vector<int> edge_list_ptr = {0, 5};

  EXPECT_FALSE(tapa::regression::PadSparseEdgeLists(edge_lists, edge_list_ptr,
                                                    /*beats_per_channel=*/4));
  EXPECT_EQ(edge_lists[0].size(), 1);
  EXPECT_EQ(edge_list_ptr.back(), 5);
}

}  // namespace
