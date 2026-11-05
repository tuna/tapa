// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <cstdlib>

#include <algorithm>
#include <functional>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <glog/logging.h>

namespace nxgraph {

template <typename Vid>
struct Edge {
  Vid src;
  Vid dst;
};

template <typename Vid, typename Eid>
struct Partition {
  using EdgeType = Edge<Vid>;
  Vid base_vid;
  Vid num_vertices;
  Eid num_edges;
  std::unique_ptr<EdgeType, std::function<void(EdgeType*)>> shard;
};

// Processes text between begin_ptr and end_ptr and return the edge array as a
// unique_ptr. If max_vid or min_vid is not nullptr, it will be updated.
template <typename Vid>
inline std::vector<Edge<Vid>> ProcessEdgeList(const char* begin_ptr,
                                              const char* end_ptr,
                                              Vid* max_vid = nullptr,
                                              Vid* min_vid = nullptr) {
  using std::numeric_limits;
  using std::string;
  using std::strtoull;
  using std::unique_ptr;
  using std::vector;
  using EdgeType = Edge<Vid>;
  if (max_vid != nullptr) {
    *max_vid = numeric_limits<Vid>::min();
  }
  if (min_vid != nullptr) {
    *min_vid = numeric_limits<Vid>::max();
  }
  const char* next_pos = nullptr;
  vector<EdgeType> edges;
  for (const char* ptr = begin_ptr; ptr < end_ptr; ++ptr) {
    // Skip spaces and tabs.
    for (; isblank(*ptr); ++ptr) {
      if (ptr >= end_ptr) {
        break;
      }
    }

    /// Skip comment lines.
    for (; *ptr == '#'; ++ptr) {
      for (; *ptr != '\n' && *ptr != '\r'; ++ptr) {
        if (ptr >= end_ptr) {
          break;
        }
      }
      for (; isblank(*ptr); ++ptr) {
        if (ptr >= end_ptr) {
          break;
        }
      }
      if (ptr >= end_ptr) {
        break;
      }
    }

    /// Swallows empty lines.
    const Vid src = strtoull(ptr, const_cast<char**>(&next_pos), 10);
    if (next_pos == ptr) {
      break;
    }

    /// Skip non-digits.
    for (; !isdigit(*next_pos); ++next_pos) {
      if (next_pos >= end_ptr) {
        break;
      }
    }

    const Vid dst = strtoull(next_pos, const_cast<char**>(&ptr), 10);
    if (next_pos == ptr) {
      break;
    }

    /// Skip spaces and tabs.
    for (; isblank(*ptr); ++ptr) {
      if (ptr >= end_ptr) {
        break;
      }
    }

    EdgeType edge{src, dst};
    if (max_vid != nullptr) {
      *max_vid = std::max({*max_vid, src, dst});
    }
    if (min_vid != nullptr) {
      *min_vid = std::min({*min_vid, src, dst});
    }
    edges.push_back(edge);
  }
  return edges;
}

template <typename Vid, typename Eid>
std::vector<Partition<Vid, Eid>> LoadEdgeList(const std::string& filename,
                                              Vid partition_size) {
  using std::runtime_error;
  using PartitionType = Partition<Vid, Eid>;
  using EdgeType = Edge<Vid>;

  int fd = open(filename.c_str(), O_RDONLY);
  if (fd == -1) {
    throw runtime_error("cannot open file " + filename);
  }

  struct stat sb;
  if (fstat(fd, &sb) == -1) {
    throw runtime_error("failed to stat " + filename);
  }

  const size_t mmap_length = sb.st_size;
  auto mmap_ptr = reinterpret_cast<char*>(
      mmap(nullptr, mmap_length, PROT_READ, MAP_SHARED, fd, /*offset=*/0));
  if (mmap_ptr == MAP_FAILED) {
    throw runtime_error("failed to mmap " + filename);
  }

  Vid max_vid;
  Vid min_vid;
  std::vector<EdgeType> edges = ProcessEdgeList<Vid>(
      mmap_ptr, mmap_ptr + mmap_length, &max_vid, &min_vid);
  Vid num_vertices = max_vid - min_vid + 1;
  VLOG(8) << "max: " << max_vid << " min: " << min_vid
          << " num vertices: " << num_vertices;

  const size_t num_partitions = (num_vertices - 1) / partition_size + 1;
  std::vector<std::vector<EdgeType>*> shards(num_partitions);
  for (auto& shard : shards) {
    shard = new std::vector<EdgeType>;
  }
  for (const auto& edge : edges) {
    VLOG(10) << "src: " << edge.src << " dst: " << edge.dst;
    const size_t pid = (edge.src - min_vid) % num_partitions;
    auto map = [&](Vid vid) -> Vid {
      // do not use 0 as vid
      return std::max(Vid(1), min_vid) +
             (vid - min_vid) % num_partitions * partition_size +
             (vid - min_vid) / num_partitions;
    };
    shards[pid]->push_back({map(edge.src), map(edge.dst)});
  }

  std::vector<PartitionType> partitions;
  partitions.reserve(num_partitions);
  for (size_t i = 0; i < num_partitions; ++i) {
    Vid base_vid = min_vid + partition_size * i;
    auto shard = shards[i];
    auto shard_ptr = shards[i]->data();
    auto shard_deleter = [shard](EdgeType*) { delete shard; };
    partitions.push_back({base_vid,
                          Vid(partition_size),
                          Eid(shards[i]->size()),
                          {shard_ptr, shard_deleter}});
  }

  if (munmap(mmap_ptr, mmap_length) != 0) {
    throw runtime_error("failed to munmap " + filename);
  }

  if (close(fd) != 0) {
    throw runtime_error("failed to close " + filename);
  }

  return partitions;
}

}  // namespace nxgraph
