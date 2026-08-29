// Index math shared by both translation units of the multi-file app. This
// header deliberately lives OUTSIDE tests/apps/multi-file: with both TUs in
// that directory it is the mirror-tree src root, so reaching this header
// only via -I exercises the out-of-root `_external/` bucket.

#pragma once

#include <cstdint>

inline uint64_t CeilDiv(uint64_t num, uint64_t den) {
  return (num + den - 1) / den;
}
