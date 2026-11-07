// Port-only fixture for `Store` of tests/functional/frame-views: the
// interface Vitis HLS 2025.2 synthesized, outputs tied to zero (see
// PROVENANCE.md).
`timescale 1 ns / 1 ps

module Store (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        m_axi_dst_AWVALID,
        m_axi_dst_AWREADY,
        m_axi_dst_AWADDR,
        m_axi_dst_AWID,
        m_axi_dst_AWLEN,
        m_axi_dst_AWSIZE,
        m_axi_dst_AWBURST,
        m_axi_dst_AWLOCK,
        m_axi_dst_AWCACHE,
        m_axi_dst_AWPROT,
        m_axi_dst_AWQOS,
        m_axi_dst_AWREGION,
        m_axi_dst_AWUSER,
        m_axi_dst_WVALID,
        m_axi_dst_WREADY,
        m_axi_dst_WDATA,
        m_axi_dst_WSTRB,
        m_axi_dst_WLAST,
        m_axi_dst_WID,
        m_axi_dst_WUSER,
        m_axi_dst_ARVALID,
        m_axi_dst_ARREADY,
        m_axi_dst_ARADDR,
        m_axi_dst_ARID,
        m_axi_dst_ARLEN,
        m_axi_dst_ARSIZE,
        m_axi_dst_ARBURST,
        m_axi_dst_ARLOCK,
        m_axi_dst_ARCACHE,
        m_axi_dst_ARPROT,
        m_axi_dst_ARQOS,
        m_axi_dst_ARREGION,
        m_axi_dst_ARUSER,
        m_axi_dst_RVALID,
        m_axi_dst_RREADY,
        m_axi_dst_RDATA,
        m_axi_dst_RLAST,
        m_axi_dst_RID,
        m_axi_dst_RUSER,
        m_axi_dst_RRESP,
        m_axi_dst_BVALID,
        m_axi_dst_BREADY,
        m_axi_dst_BRESP,
        m_axi_dst_BID,
        m_axi_dst_BUSER,
        in_s_dout,
        in_s_empty_n,
        in_s_read,
        in_peek_dout,
        in_peek_empty_n,
        in_peek_read,
        dst_r,
        n
);

parameter    ap_ST_fsm_state1 = 9'd1;
parameter    ap_ST_fsm_state2 = 9'd2;
parameter    ap_ST_fsm_state3 = 9'd4;
parameter    ap_ST_fsm_state4 = 9'd8;
parameter    ap_ST_fsm_state5 = 9'd16;
parameter    ap_ST_fsm_state6 = 9'd32;
parameter    ap_ST_fsm_state7 = 9'd64;
parameter    ap_ST_fsm_state8 = 9'd128;
parameter    ap_ST_fsm_state9 = 9'd256;
parameter    C_M_AXI_DST_ID_WIDTH = 1;
parameter    C_M_AXI_DST_ADDR_WIDTH = 64;
parameter    C_M_AXI_DST_DATA_WIDTH = 32;
parameter    C_M_AXI_DST_AWUSER_WIDTH = 1;
parameter    C_M_AXI_DST_ARUSER_WIDTH = 1;
parameter    C_M_AXI_DST_WUSER_WIDTH = 1;
parameter    C_M_AXI_DST_RUSER_WIDTH = 1;
parameter    C_M_AXI_DST_BUSER_WIDTH = 1;
parameter    C_M_AXI_DST_USER_VALUE = 0;
parameter    C_M_AXI_DST_PROT_VALUE = 0;
parameter    C_M_AXI_DST_CACHE_VALUE = 3;
parameter    C_M_AXI_DATA_WIDTH = 32;
parameter C_M_AXI_DST_WSTRB_WIDTH = (32 / 8);
parameter C_M_AXI_WSTRB_WIDTH = (32 / 8);
input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
output   m_axi_dst_AWVALID;
input   m_axi_dst_AWREADY;
output  [C_M_AXI_DST_ADDR_WIDTH - 1:0] m_axi_dst_AWADDR;
output  [C_M_AXI_DST_ID_WIDTH - 1:0] m_axi_dst_AWID;
output  [7:0] m_axi_dst_AWLEN;
output  [2:0] m_axi_dst_AWSIZE;
output  [1:0] m_axi_dst_AWBURST;
output  [1:0] m_axi_dst_AWLOCK;
output  [3:0] m_axi_dst_AWCACHE;
output  [2:0] m_axi_dst_AWPROT;
output  [3:0] m_axi_dst_AWQOS;
output  [3:0] m_axi_dst_AWREGION;
output  [C_M_AXI_DST_AWUSER_WIDTH - 1:0] m_axi_dst_AWUSER;
output   m_axi_dst_WVALID;
input   m_axi_dst_WREADY;
output  [C_M_AXI_DST_DATA_WIDTH - 1:0] m_axi_dst_WDATA;
output  [C_M_AXI_DST_WSTRB_WIDTH - 1:0] m_axi_dst_WSTRB;
output   m_axi_dst_WLAST;
output  [C_M_AXI_DST_ID_WIDTH - 1:0] m_axi_dst_WID;
output  [C_M_AXI_DST_WUSER_WIDTH - 1:0] m_axi_dst_WUSER;
output   m_axi_dst_ARVALID;
input   m_axi_dst_ARREADY;
output  [C_M_AXI_DST_ADDR_WIDTH - 1:0] m_axi_dst_ARADDR;
output  [C_M_AXI_DST_ID_WIDTH - 1:0] m_axi_dst_ARID;
output  [7:0] m_axi_dst_ARLEN;
output  [2:0] m_axi_dst_ARSIZE;
output  [1:0] m_axi_dst_ARBURST;
output  [1:0] m_axi_dst_ARLOCK;
output  [3:0] m_axi_dst_ARCACHE;
output  [2:0] m_axi_dst_ARPROT;
output  [3:0] m_axi_dst_ARQOS;
output  [3:0] m_axi_dst_ARREGION;
output  [C_M_AXI_DST_ARUSER_WIDTH - 1:0] m_axi_dst_ARUSER;
input   m_axi_dst_RVALID;
output   m_axi_dst_RREADY;
input  [C_M_AXI_DST_DATA_WIDTH - 1:0] m_axi_dst_RDATA;
input   m_axi_dst_RLAST;
input  [C_M_AXI_DST_ID_WIDTH - 1:0] m_axi_dst_RID;
input  [C_M_AXI_DST_RUSER_WIDTH - 1:0] m_axi_dst_RUSER;
input  [1:0] m_axi_dst_RRESP;
input   m_axi_dst_BVALID;
output   m_axi_dst_BREADY;
input  [1:0] m_axi_dst_BRESP;
input  [C_M_AXI_DST_ID_WIDTH - 1:0] m_axi_dst_BID;
input  [C_M_AXI_DST_BUSER_WIDTH - 1:0] m_axi_dst_BUSER;
input  [32:0] in_s_dout;
input   in_s_empty_n;
output   in_s_read;
input  [32:0] in_peek_dout;
input   in_peek_empty_n;
output   in_peek_read;
input  [63:0] dst_r;
input  [31:0] n;

assign ap_done = 'd0;
assign ap_idle = 'd0;
assign ap_ready = 'd0;
assign m_axi_dst_AWVALID = 'd0;
assign m_axi_dst_AWADDR = 'd0;
assign m_axi_dst_AWID = 'd0;
assign m_axi_dst_AWLEN = 'd0;
assign m_axi_dst_AWSIZE = 'd0;
assign m_axi_dst_AWBURST = 'd0;
assign m_axi_dst_AWLOCK = 'd0;
assign m_axi_dst_AWCACHE = 'd0;
assign m_axi_dst_AWPROT = 'd0;
assign m_axi_dst_AWQOS = 'd0;
assign m_axi_dst_AWREGION = 'd0;
assign m_axi_dst_AWUSER = 'd0;
assign m_axi_dst_WVALID = 'd0;
assign m_axi_dst_WDATA = 'd0;
assign m_axi_dst_WSTRB = 'd0;
assign m_axi_dst_WLAST = 'd0;
assign m_axi_dst_WID = 'd0;
assign m_axi_dst_WUSER = 'd0;
assign m_axi_dst_ARVALID = 'd0;
assign m_axi_dst_ARADDR = 'd0;
assign m_axi_dst_ARID = 'd0;
assign m_axi_dst_ARLEN = 'd0;
assign m_axi_dst_ARSIZE = 'd0;
assign m_axi_dst_ARBURST = 'd0;
assign m_axi_dst_ARLOCK = 'd0;
assign m_axi_dst_ARCACHE = 'd0;
assign m_axi_dst_ARPROT = 'd0;
assign m_axi_dst_ARQOS = 'd0;
assign m_axi_dst_ARREGION = 'd0;
assign m_axi_dst_ARUSER = 'd0;
assign m_axi_dst_RREADY = 'd0;
assign m_axi_dst_BREADY = 'd0;
assign in_s_read = 'd0;
assign in_peek_read = 'd0;

endmodule
