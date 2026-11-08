// Port-only fixture for `Drain` of tests/functional/frame-views: the
// interface Vitis HLS 2025.2 synthesized, outputs tied to zero (see
// PROVENANCE.md).
`timescale 1 ns / 1 ps

module Drain (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        turned_s_dout,
        turned_s_empty_n,
        turned_s_read,
        turned_peek_dout,
        turned_peek_empty_n,
        turned_peek_read,
        dst_offset,
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
input  [32:0] turned_s_dout;
input   turned_s_empty_n;
output   turned_s_read;
input  [32:0] turned_peek_dout;
input   turned_peek_empty_n;
output   turned_peek_read;
input  [63:0] dst_offset;
input  [31:0] n;

assign ap_done = 'd0;
assign ap_idle = 'd0;
assign ap_ready = 'd0;
assign turned_s_read = 'd0;
assign turned_peek_read = 'd0;

endmodule
