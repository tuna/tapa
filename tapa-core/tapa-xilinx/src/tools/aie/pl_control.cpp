#include "ap_axi_sdata.h"
#include "ap_int.h"
#include "hls_stream.h"

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

extern "C" void tapa_aie_pl_control(hls::stream<Packet>& output, ap_uint<32> id,
                                    ap_uint<32>* program) {
#pragma HLS interface axis port = output
#pragma HLS interface s_axilite port = id bundle = control
#pragma HLS interface m_axi port = program offset = slave bundle = gmem
#pragma HLS interface s_axilite port = program bundle = control
#pragma HLS interface s_axilite port = return bundle = control

  ap_uint<32> commands[1024];
  const ap_uint<32> words = program[0];
  for (ap_uint<32> i = 0; i < 1024; ++i) {
#pragma HLS pipeline II = 1
    if (i < words) commands[i] = program[i + 1];
  }
  for (ap_uint<32> pc = 0; pc + 1 < words; pc += 2) {
    const ap_uint<32> address = commands[pc];
    const ap_uint<32> value = commands[pc + 1];
    write_register(output, id, address, value);
  }
}
