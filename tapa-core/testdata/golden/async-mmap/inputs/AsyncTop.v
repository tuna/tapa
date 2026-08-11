// HLS-style input fixture for the `AsyncTop` upper task of
// `tests/apps/async_mmap/async_mmap.cpp` (see PROVENANCE.md). Same shape as
// the `vadd` top fixture, plus the external output stream `data_q` that the
// child drives (HLS spells external streams as plain `out_*` ports).
`timescale 1 ns / 1 ps

module AsyncTop (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        s_axi_control_AWVALID,
        s_axi_control_AWREADY,
        s_axi_control_AWADDR,
        s_axi_control_WVALID,
        s_axi_control_WREADY,
        s_axi_control_WDATA,
        s_axi_control_WSTRB,
        s_axi_control_ARVALID,
        s_axi_control_ARREADY,
        s_axi_control_ARADDR,
        s_axi_control_RVALID,
        s_axi_control_RREADY,
        s_axi_control_RDATA,
        s_axi_control_RRESP,
        s_axi_control_BVALID,
        s_axi_control_BREADY,
        s_axi_control_BRESP,
        interrupt,
        data_q_din,
        data_q_full_n,
        data_q_write
);

parameter    C_S_AXI_CONTROL_DATA_WIDTH = 32;
parameter    C_S_AXI_CONTROL_ADDR_WIDTH = 6;

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input   s_axi_control_AWVALID;
output   s_axi_control_AWREADY;
input  [5:0] s_axi_control_AWADDR;
input   s_axi_control_WVALID;
output   s_axi_control_WREADY;
input  [31:0] s_axi_control_WDATA;
input  [3:0] s_axi_control_WSTRB;
input   s_axi_control_ARVALID;
output   s_axi_control_ARREADY;
input  [5:0] s_axi_control_ARADDR;
output   s_axi_control_RVALID;
input   s_axi_control_RREADY;
output  [31:0] s_axi_control_RDATA;
output  [1:0] s_axi_control_RRESP;
output   s_axi_control_BVALID;
input   s_axi_control_BREADY;
output  [1:0] s_axi_control_BRESP;
output   interrupt;
output  [32:0] data_q_din;
input   data_q_full_n;
output   data_q_write;

reg ap_done;
reg ap_idle;
reg ap_ready;
reg interrupt;

endmodule
