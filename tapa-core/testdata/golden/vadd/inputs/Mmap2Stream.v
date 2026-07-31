// HLS-style input fixture for the `Mmap2Stream` lower task of
// `tests/apps/vadd` (see PROVENANCE.md). An HLS mmap argument realizes a full
// compact M-AXI master plus a 64-bit offset input; the ostream follows the
// same spelling as producer streams elsewhere.
`timescale 1 ns / 1 ps

module Mmap2Stream (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        mmap_port_offset,
        m_axi_mmap_port_AWVALID,
        m_axi_mmap_port_AWREADY,
        m_axi_mmap_port_AWADDR,
        m_axi_mmap_port_AWID,
        m_axi_mmap_port_AWLEN,
        m_axi_mmap_port_AWSIZE,
        m_axi_mmap_port_AWBURST,
        m_axi_mmap_port_WVALID,
        m_axi_mmap_port_WREADY,
        m_axi_mmap_port_WDATA,
        m_axi_mmap_port_WSTRB,
        m_axi_mmap_port_WLAST,
        m_axi_mmap_port_BVALID,
        m_axi_mmap_port_BREADY,
        m_axi_mmap_port_BID,
        m_axi_mmap_port_BRESP,
        m_axi_mmap_port_ARVALID,
        m_axi_mmap_port_ARREADY,
        m_axi_mmap_port_ARADDR,
        m_axi_mmap_port_ARID,
        m_axi_mmap_port_ARLEN,
        m_axi_mmap_port_ARSIZE,
        m_axi_mmap_port_ARBURST,
        m_axi_mmap_port_RVALID,
        m_axi_mmap_port_RREADY,
        m_axi_mmap_port_RDATA,
        m_axi_mmap_port_RID,
        m_axi_mmap_port_RLAST,
        m_axi_mmap_port_RRESP,
        stream_port_din,
        stream_port_full_n,
        stream_port_write
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [63:0] mmap_port_offset;
output   m_axi_mmap_port_AWVALID;
input   m_axi_mmap_port_AWREADY;
output  [63:0] m_axi_mmap_port_AWADDR;
output   m_axi_mmap_port_AWID;
output  [7:0] m_axi_mmap_port_AWLEN;
output  [2:0] m_axi_mmap_port_AWSIZE;
output  [1:0] m_axi_mmap_port_AWBURST;
output   m_axi_mmap_port_WVALID;
input   m_axi_mmap_port_WREADY;
output  [31:0] m_axi_mmap_port_WDATA;
output  [3:0] m_axi_mmap_port_WSTRB;
output   m_axi_mmap_port_WLAST;
input   m_axi_mmap_port_BVALID;
output   m_axi_mmap_port_BREADY;
input   m_axi_mmap_port_BID;
input  [1:0] m_axi_mmap_port_BRESP;
output   m_axi_mmap_port_ARVALID;
input   m_axi_mmap_port_ARREADY;
output  [63:0] m_axi_mmap_port_ARADDR;
output   m_axi_mmap_port_ARID;
output  [7:0] m_axi_mmap_port_ARLEN;
output  [2:0] m_axi_mmap_port_ARSIZE;
output  [1:0] m_axi_mmap_port_ARBURST;
input   m_axi_mmap_port_RVALID;
output   m_axi_mmap_port_RREADY;
input  [31:0] m_axi_mmap_port_RDATA;
input   m_axi_mmap_port_RID;
input   m_axi_mmap_port_RLAST;
input  [1:0] m_axi_mmap_port_RRESP;
output  [32:0] stream_port_din;
input   stream_port_full_n;
output   stream_port_write;

reg ap_done;
reg ap_idle;
reg ap_ready;

endmodule
