// HLS-style input fixture for the `AsyncReader` lower task of
// `tests/apps/async_mmap/async_mmap.cpp` (see PROVENANCE.md). Vitis HLS
// realizes a `tapa::async_mmap` argument as five FIFO-style channels; this
// reader only activates read_addr/read_data, and its unused write-side
// activity outputs are tied to zero — the exact pattern TAPA codegen uses
// to prune the write half of the bridge.
`timescale 1 ns / 1 ps

module AsyncReader (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        mem_read_addr_s_din,
        mem_read_addr_s_full_n,
        mem_read_addr_s_write,
        mem_read_addr_offset,
        mem_read_data_s_dout,
        mem_read_data_s_empty_n,
        mem_read_data_s_read,
        mem_read_data_peek_dout,
        mem_read_data_peek_empty_n,
        mem_read_data_peek_read,
        mem_write_addr_s_write,
        mem_write_data_s_write,
        mem_write_resp_s_read,
        n,
        data_q_din,
        data_q_full_n,
        data_q_write
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
output  [63:0] mem_read_addr_s_din;
input   mem_read_addr_s_full_n;
output   mem_read_addr_s_write;
input  [63:0] mem_read_addr_offset;
input  [32:0] mem_read_data_s_dout;
input   mem_read_data_s_empty_n;
output   mem_read_data_s_read;
input  [32:0] mem_read_data_peek_dout;
input   mem_read_data_peek_empty_n;
output   mem_read_data_peek_read;
output   mem_write_addr_s_write;
output   mem_write_data_s_write;
output   mem_write_resp_s_read;
input  [63:0] n;
output  [32:0] data_q_din;
input   data_q_full_n;
output   data_q_write;

reg ap_done;
reg ap_idle;
reg ap_ready;

assign mem_write_addr_s_write = 1'b0;
assign mem_write_data_s_write = 1'b0;
assign mem_write_resp_s_read = 1'b0;

endmodule
