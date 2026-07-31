// HLS-style input fixture for the `Add` lower task of `tests/apps/vadd`
// (see PROVENANCE.md). Scalar/istream/ostream interfaces follow the Vitis HLS
// spellings: stream payloads are one bit wider than the element type (EOT),
// and consumer streams carry the peek trio.
`timescale 1 ns / 1 ps

module Add (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        n,
        a_dout,
        a_empty_n,
        a_read,
        a_peek_dout,
        a_peek_empty_n,
        a_peek_read,
        b_dout,
        b_empty_n,
        b_read,
        b_peek_dout,
        b_peek_empty_n,
        b_peek_read,
        c_din,
        c_full_n,
        c_write
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [63:0] n;
input  [32:0] a_dout;
input   a_empty_n;
output   a_read;
input  [32:0] a_peek_dout;
input   a_peek_empty_n;
output   a_peek_read;
input  [32:0] b_dout;
input   b_empty_n;
output   b_read;
input  [32:0] b_peek_dout;
input   b_peek_empty_n;
output   b_peek_read;
output  [32:0] c_din;
input   c_full_n;
output   c_write;

reg ap_done;
reg ap_idle;
reg ap_ready;
reg a_read;
reg a_peek_read;
reg b_read;
reg b_peek_read;

endmodule
