#include "ap_axi_sdata.h"
#include "ap_int.h"
#include "hls_stream.h"
#if __has_include("ap_utils.h")
#include "ap_utils.h"
#else
#include "etc/ap_utils.h"
#endif

// `kTiles`, and per controlled tile `kTileKeys[i]` ((column << 16) | row),
// `kTilePackets[i]`, the packet id aiecompiler routes to that tile's packet
// controller, and `kTileReturns[i]`, the one it answers a read on; then
// `kResetSteps` (tile, address, value) triples `kReset`, which restart a
// run-forever graph's buffers, each a read where the tile's bit 31 is set.
// Generated for each design (`pl_control::compile`).
#include "pl_control_tiles.h"

namespace {

using Packet = ap_axiu<32, 0, 0, 0>;

// An AIE1 core's control register: bit 1 resets the core, bit 0 enables it.
constexpr unsigned kCoreControl = 0x32000;
constexpr unsigned kCoreReset = 2;

ap_uint<32> with_odd_parity(ap_uint<32> value) {
#pragma HLS inline
  value[31] = !value(30, 0).xor_reduce();
  return value;
}

// Sends the header and control word of a packet to the tile packet
// controller `id`: operation `op` (0 writes, 1 reads) on the word at
// `address`, the answer to a read returning on packet id `ret`.
void send_control(hls::stream<Packet>& output, ap_uint<32> id, ap_uint<2> op,
                  ap_uint<32> ret, ap_uint<32> address, bool last) {
#pragma HLS inline
  Packet packet;
  packet.keep = -1;
  packet.last = 0;
  packet.data = with_odd_parity((id & 0x1f) | (0x1f << 16) | (0x7f << 21));
  output.write(packet);
  packet.data = with_odd_parity((address & 0x3ffff) | (ap_uint<32>(op) << 20) |
                                ((ret & 0x1f) << 24));
  packet.last = last;
  output.write(packet);
}

void write_register(hls::stream<Packet>& output, ap_uint<32> id,
                    ap_uint<32> address, ap_uint<32> value) {
#pragma HLS inline
  send_control(output, id, 0, 0, address, false);
  Packet packet;
  packet.keep = -1;
  packet.data = value;
  packet.last = 1;
  output.write(packet);
}

// Reads `address`, by which an AIE1 tile performs a lock operation, and
// drops the answer: a header, then the one word read.
void read_register(hls::stream<Packet>& output,
                   hls::stream<ap_uint<32>>& response, ap_uint<32> id,
                   ap_uint<32> ret, ap_uint<32> address) {
#pragma HLS inline
  send_control(output, id, 1, ret, address, true);
  // The answer depends on the request only through the AI Engine, so HLS
  // would read it as early as the request's own writes: keep it after them.
  ap_wait();
  response.read();
  response.read();
}

// Writes `value` to `address` of the tile keyed `tile`, or reads `address`
// when `read` is set.
void access(hls::stream<Packet>& output, hls::stream<ap_uint<32>>& response,
            ap_uint<32> tile, ap_uint<32> address, ap_uint<32> value,
            bool read) {
  for (int i = 0; i < kTiles; ++i) {
#pragma HLS pipeline off
    if (kTileKeys[i] == tile) {
      if (read) {
        read_register(output, response, kTilePackets[i], kTileReturns[i],
                      address);
      } else {
        write_register(output, kTilePackets[i], address, value);
      }
    }
  }
}

}  // namespace

// With `reset` set, the graph restarts first: its cores are reset, then
// `kReset` restarts its buffers. Then `program[0]` counts the words after
// it: (tile, address, value) triples, each written to the named tile only.
// The host names controlled tiles only. The program is copied in chunks of
// whole triples, so its length is bounded only by the 32-bit count.
// `response` carries the tiles' answers as bare words: an AI Engine stream
// has no TKEEP or TSTRB, which `ap_axiu` would add and which `v++ --link`
// would bridge with a converter whose AI Engine clock it leaves unconnected.
extern "C" void tapa_aie_pl_control(hls::stream<Packet>& output,
                                    ap_uint<32>* program, ap_uint<32> reset,
                                    hls::stream<ap_uint<32>>& response) {
#pragma HLS interface axis port = output
#pragma HLS interface m_axi port = program offset = slave bundle = gmem
#pragma HLS interface s_axilite port = program bundle = control
#pragma HLS interface s_axilite port = reset bundle = control
#pragma HLS interface axis port = response
#pragma HLS interface s_axilite port = return bundle = control

  if (reset != 0) {
    for (int i = 0; i < kTiles; ++i) {
      write_register(output, kTilePackets[i], kCoreControl, kCoreReset);
    }
    for (int i = 0; i < kTiles; ++i) {
      write_register(output, kTilePackets[i], kCoreControl, 0);
    }
    // One step at a time: a read waits for its answer before the next
    // step's request.
    for (int step = 0; step < kResetSteps; ++step) {
#pragma HLS pipeline off
      const ap_uint<32> tile = kReset[step][0];
      access(output, response, tile & 0x7fffffff, kReset[step][1],
             kReset[step][2], tile[31]);
    }
  }

  constexpr int kChunk = 1023;
  ap_uint<32> commands[kChunk];
  const ap_uint<33> words = program[0];
  for (ap_uint<33> base = 0; base < words; base += kChunk) {
    const ap_uint<33> left = words - base;
    const ap_uint<11> count = left < kChunk ? left : ap_uint<33>(kChunk);
    for (ap_uint<11> i = 0; i < kChunk; ++i) {
#pragma HLS pipeline II = 1
      if (i < count) commands[i] = program[base + i + 1];
    }
    for (ap_uint<11> pc = 0; pc + 2 < count; pc += 3) {
      access(output, response, commands[pc], commands[pc + 1], commands[pc + 2],
             false);
    }
  }
}
