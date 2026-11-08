// Port-only fixture for `Turn` of tests/functional/frame-views: the
// interface Vitis HLS 2025.2 synthesized, outputs tied to zero (see
// PROVENANCE.md).
`timescale 1 ns / 1 ps

module Turn (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        in_buf_address0,
        in_buf_ce0,
        in_buf_q0,
        in_acquire_dout,
        in_acquire_empty_n,
        in_acquire_read,
        in_release_din,
        in_release_full_n,
        in_release_write,
        out_buf_address0,
        out_buf_ce0,
        out_buf_we0,
        out_buf_d0,
        out_acquire_dout,
        out_acquire_empty_n,
        out_acquire_read,
        out_release_din,
        out_release_full_n,
        out_release_write,
        n
);

parameter    ap_ST_fsm_state1 = 6'd1;
parameter    ap_ST_fsm_state2 = 6'd2;
parameter    ap_ST_fsm_state3 = 6'd4;
parameter    ap_ST_fsm_state4 = 6'd8;
parameter    ap_ST_fsm_state5 = 6'd16;
parameter    ap_ST_fsm_state6 = 6'd32;
input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
output  [17:0] in_buf_address0;
output   in_buf_ce0;
input  [31:0] in_buf_q0;
input  [7:0] in_acquire_dout;
input   in_acquire_empty_n;
output   in_acquire_read;
output  [0:0] in_release_din;
input   in_release_full_n;
output   in_release_write;
output  [17:0] out_buf_address0;
output   out_buf_ce0;
output   out_buf_we0;
output  [31:0] out_buf_d0;
input  [7:0] out_acquire_dout;
input   out_acquire_empty_n;
output   out_acquire_read;
output  [0:0] out_release_din;
input   out_release_full_n;
output   out_release_write;
input  [31:0] n;

assign ap_done = 'd0;
assign ap_idle = 'd0;
assign ap_ready = 'd0;
assign in_buf_address0 = 'd0;
assign in_buf_ce0 = 'd0;
assign in_acquire_read = 'd0;
assign in_release_din = 'd0;
assign in_release_write = 'd0;
assign out_buf_address0 = 'd0;
assign out_buf_ce0 = 'd0;
assign out_buf_we0 = 'd0;
assign out_buf_d0 = 'd0;
assign out_acquire_read = 'd0;
assign out_release_din = 'd0;
assign out_release_write = 'd0;

endmodule
