// Port-only fixture for `Emit` of tests/functional/frame-views: the
// interface Vitis HLS 2025.2 synthesized, outputs tied to zero (see
// PROVENANCE.md).
`timescale 1 ns / 1 ps

module Emit (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        once_s_dout,
        once_s_empty_n,
        once_s_read,
        once_peek_dout,
        once_peek_empty_n,
        once_peek_read,
        twice_s_din,
        twice_s_full_n,
        twice_s_write,
        twice_peek,
        n
);

parameter    ap_ST_fsm_state1 = 2'd1;
parameter    ap_ST_fsm_state2 = 2'd2;
input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [32:0] once_s_dout;
input   once_s_empty_n;
output   once_s_read;
input  [32:0] once_peek_dout;
input   once_peek_empty_n;
output   once_peek_read;
output  [32:0] twice_s_din;
input   twice_s_full_n;
output   twice_s_write;
input  [32:0] twice_peek;
input  [31:0] n;

assign ap_done = 'd0;
assign ap_idle = 'd0;
assign ap_ready = 'd0;
assign once_s_read = 'd0;
assign once_peek_read = 'd0;
assign twice_s_din = 'd0;
assign twice_s_write = 'd0;

endmodule
