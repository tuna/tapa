// HLS-style input fixture for the `ShellTop` upper task of the
// `ignore-shell` golden case (see PROVENANCE.md). Same shape as the
// `async-mmap` top fixture without the m_axi ports: `ShellTop` carries
// only the scalar `n` (delivered through s_axi_control) and the external
// output stream `out` that its `target("ignore")` child drives.
`timescale 1 ns / 1 ps

module ShellTop (
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
        out_din,
        out_full_n,
        out_write
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
output  [32:0] out_din;
input   out_full_n;
output   out_write;

reg ap_done;
reg ap_idle;
reg ap_ready;
reg interrupt;

endmodule
