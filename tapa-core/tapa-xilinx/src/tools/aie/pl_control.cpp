#include "ap_axi_sdata.h"
#include "ap_int.h"
#include "hls_stream.h"

// `kTiles`, and per controlled tile `kTileKeys[i]` ((column << 16) | row)
// and `kTilePackets[i]`, the packet id aiecompiler routes to that tile's
// packet controller; generated for each design (`pl_control::compile`).
#include "pl_control_tiles.h"

namespace {

using Packet = ap_axiu<32, 0, 0, 0>;

ap_uint<32> with_odd_parity(ap_uint<32> value) {
#pragma HLS inline
  value[31] = !value(30, 0).xor_reduce();
  return value;
}

void write_register(hls::stream<Packet>& output, ap_uint<32> id,
                    ap_uint<32> address, ap_uint<32> value) {
#pragma HLS inline
  Packet packet;
  packet.keep = -1;
  packet.last = 0;
  packet.data = with_odd_parity((id & 0x1f) | (0x1f << 16) | (0x7f << 21));
  output.write(packet);
  packet.data = with_odd_parity(address & 0x3ffff);
  output.write(packet);
  packet.data = value;
  packet.last = 1;
  output.write(packet);
}

}  // namespace

// `program[0]` counts the words after it: (tile, address, value) triples,
// each written to the named tile only. The host names controlled tiles only.
// The program is copied in chunks of whole triples, so its length is bounded
// only by the 32-bit count.
extern "C" void tapa_aie_pl_control(hls::stream<Packet>& output,
                                    ap_uint<32>* program) {
#pragma HLS interface axis port = output
#pragma HLS interface m_axi port = program offset = slave bundle = gmem
#pragma HLS interface s_axilite port = program bundle = control
#pragma HLS interface s_axilite port = return bundle = control

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
      const ap_uint<32> tile = commands[pc];
      for (int i = 0; i < kTiles; ++i) {
        if (kTileKeys[i] == tile) {
          write_register(output, kTilePackets[i], commands[pc + 1],
                         commands[pc + 2]);
        }
      }
    }
  }
}
