#include "xrt/xrt_graph.h"

#include <cerrno>
#include <cstdint>
#include <cstring>
#include <new>
#include <string>

struct xrtHandles {
  xclDeviceHandle device;
  xrtGraphHandle graph;
  xuid_t uuid;
};

namespace {

constexpr char kGraphName[] = "@GRAPH_NAME@";
constexpr uint32_t kStart = 0;
constexpr uint32_t kStop = 1;
constexpr uint32_t kAbort = 2;

uint32_t read_u32(const uint8_t*& cursor, const uint8_t* end, bool& valid) {
  if (static_cast<size_t>(end - cursor) < sizeof(uint32_t)) {
    valid = false;
    return 0;
  }
  uint32_t value;
  std::memcpy(&value, cursor, sizeof(value));
  cursor += sizeof(value);
  return value;
}

uint64_t read_u64(const uint8_t*& cursor, const uint8_t* end, bool& valid) {
  if (static_cast<size_t>(end - cursor) < sizeof(uint64_t)) {
    valid = false;
    return 0;
  }
  uint64_t value;
  std::memcpy(&value, cursor, sizeof(value));
  cursor += sizeof(value);
  return value;
}

int start(const uint8_t* cursor, const uint8_t* end, xrtHandles* handles) {
  bool valid = true;
  const bool reset = read_u32(cursor, end, valid) != 0;
  const int iterations = static_cast<int32_t>(read_u32(cursor, end, valid));
  const uint32_t rtp_count = read_u32(cursor, end, valid);
  if (!valid) return EINVAL;
  if (reset) {
    const int rc = xrtGraphReset(handles->graph);
    if (rc != 0) return rc;
  }
  for (uint32_t i = 0; i < rtp_count; ++i) {
    const uint32_t name_bytes = read_u32(cursor, end, valid);
    const uint32_t value_bytes = read_u32(cursor, end, valid);
    if (!valid || name_bytes == 0 || end - cursor < name_bytes) return EINVAL;
    const std::string name(reinterpret_cast<const char*>(cursor), name_bytes);
    cursor += name_bytes;
    if (end - cursor < value_bytes ||
        std::memchr(name.data(), '\0', name.size()) != nullptr) {
      return EINVAL;
    }
    const int rc =
        xrtGraphUpdateRTP(handles->graph, name.c_str(),
                          reinterpret_cast<const char*>(cursor), value_bytes);
    if (rc != 0) return rc;
    cursor += value_bytes;
  }
  if (cursor != end) return EINVAL;
  return xrtGraphRun(handles->graph, iterations);
}

}  // namespace

extern "C" {

__attribute__((visibility("default"))) xrtHandles* tapa_aie_control_init(
    xclDeviceHandle device, const xuid_t xclbin_uuid) {
  auto* handles = new (std::nothrow) xrtHandles{device, nullptr, {}};
  if (handles == nullptr) return nullptr;
  std::memcpy(handles->uuid, xclbin_uuid, sizeof(xuid_t));
  return handles;
}

__attribute__((visibility("default"))) int tapa_aie_control(
    uint8_t* commands, uint32_t bytes, xrtHandles* handles) {
  const int rc = [&]() {
    if (commands == nullptr || handles == nullptr) return EINVAL;
    if (handles->graph == nullptr) {
      handles->graph = xrtGraphOpen(handles->device, handles->uuid, kGraphName);
      if (handles->graph == nullptr) return EIO;
    }
    const uint8_t* cursor = commands;
    const uint8_t* end = commands + bytes;
    bool valid = true;
    const uint32_t action = read_u32(cursor, end, valid);
    if (!valid) return EINVAL;
    switch (action) {
      case kStart:
        return start(cursor, end, handles);
      case kStop: {
        const uint64_t cycles = read_u64(cursor, end, valid);
        if (!valid || cursor != end) return EINVAL;
        return xrtGraphWait(handles->graph, cycles);
      }
      case kAbort:
        if (cursor != end) return EINVAL;
        return xrtGraphReset(handles->graph);
      default:
        return EINVAL;
    }
  }();
  if (commands != nullptr && bytes >= sizeof(uint32_t)) {
    const uint32_t status = static_cast<uint32_t>(rc);
    std::memcpy(commands, &status, sizeof(status));
  }
  return rc;
}

__attribute__((visibility("default"))) int tapa_aie_control_fini(
    xrtHandles* handles) {
  if (handles == nullptr) return EINVAL;
  if (handles->graph != nullptr) xrtGraphClose(handles->graph);
  delete handles;
  return 0;
}

}  // extern "C"
