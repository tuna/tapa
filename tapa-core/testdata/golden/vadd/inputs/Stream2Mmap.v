// HLS-style input fixture for the `Stream2Mmap` lower task of
// `tests/apps/vadd` (see PROVENANCE.md). Mirror of `Mmap2Stream` with a
// consumer stream interface (including the peek trio).
`timescale 1 ns / 1 ps

module Stream2Mmap (
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
        stream_port_dout,
        stream_port_empty_n,
        stream_port_read,
        stream_port_peek_dout,
        stream_port_peek_empty_n,
        stream_port_peek_read
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
input  [32:0] stream_port_dout;
input   stream_port_empty_n;
output   stream_port_read;
input  [32:0] stream_port_peek_dout;
input   stream_port_peek_empty_n;
output   stream_port_peek_read;

reg ap_done;
reg ap_idle;
reg ap_ready;
reg stream_port_read;
reg stream_port_peek_read;

endmodule
