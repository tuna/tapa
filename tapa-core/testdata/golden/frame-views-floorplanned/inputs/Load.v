// Port-only fixture for `Load` of tests/functional/frame-views: the
// interface Vitis HLS 2025.2 synthesized, outputs tied to zero (see
// PROVENANCE.md).
`timescale 1 ns / 1 ps

module Load (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        m_axi_src_AWVALID,
        m_axi_src_AWREADY,
        m_axi_src_AWADDR,
        m_axi_src_AWID,
        m_axi_src_AWLEN,
        m_axi_src_AWSIZE,
        m_axi_src_AWBURST,
        m_axi_src_AWLOCK,
        m_axi_src_AWCACHE,
        m_axi_src_AWPROT,
        m_axi_src_AWQOS,
        m_axi_src_AWREGION,
        m_axi_src_AWUSER,
        m_axi_src_WVALID,
        m_axi_src_WREADY,
        m_axi_src_WDATA,
        m_axi_src_WSTRB,
        m_axi_src_WLAST,
        m_axi_src_WID,
        m_axi_src_WUSER,
        m_axi_src_ARVALID,
        m_axi_src_ARREADY,
        m_axi_src_ARADDR,
        m_axi_src_ARID,
        m_axi_src_ARLEN,
        m_axi_src_ARSIZE,
        m_axi_src_ARBURST,
        m_axi_src_ARLOCK,
        m_axi_src_ARCACHE,
        m_axi_src_ARPROT,
        m_axi_src_ARQOS,
        m_axi_src_ARREGION,
        m_axi_src_ARUSER,
        m_axi_src_RVALID,
        m_axi_src_RREADY,
        m_axi_src_RDATA,
        m_axi_src_RLAST,
        m_axi_src_RID,
        m_axi_src_RUSER,
        m_axi_src_RRESP,
        m_axi_src_BVALID,
        m_axi_src_BREADY,
        m_axi_src_BRESP,
        m_axi_src_BID,
        m_axi_src_BUSER,
        src_r,
        out_s_din,
        out_s_full_n,
        out_s_write,
        out_peek,
        n
);

parameter    ap_ST_fsm_state1 = 11'd1;
parameter    ap_ST_fsm_state2 = 11'd2;
parameter    ap_ST_fsm_state3 = 11'd4;
parameter    ap_ST_fsm_state4 = 11'd8;
parameter    ap_ST_fsm_state5 = 11'd16;
parameter    ap_ST_fsm_state6 = 11'd32;
parameter    ap_ST_fsm_state7 = 11'd64;
parameter    ap_ST_fsm_state8 = 11'd128;
parameter    ap_ST_fsm_state9 = 11'd256;
parameter    ap_ST_fsm_state10 = 11'd512;
parameter    ap_ST_fsm_state11 = 11'd1024;
parameter    C_M_AXI_SRC_ID_WIDTH = 1;
parameter    C_M_AXI_SRC_ADDR_WIDTH = 64;
parameter    C_M_AXI_SRC_DATA_WIDTH = 32;
parameter    C_M_AXI_SRC_AWUSER_WIDTH = 1;
parameter    C_M_AXI_SRC_ARUSER_WIDTH = 1;
parameter    C_M_AXI_SRC_WUSER_WIDTH = 1;
parameter    C_M_AXI_SRC_RUSER_WIDTH = 1;
parameter    C_M_AXI_SRC_BUSER_WIDTH = 1;
parameter    C_M_AXI_SRC_USER_VALUE = 0;
parameter    C_M_AXI_SRC_PROT_VALUE = 0;
parameter    C_M_AXI_SRC_CACHE_VALUE = 3;
parameter    C_M_AXI_DATA_WIDTH = 32;
parameter C_M_AXI_SRC_WSTRB_WIDTH = (32 / 8);
parameter C_M_AXI_WSTRB_WIDTH = (32 / 8);
input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
output   m_axi_src_AWVALID;
input   m_axi_src_AWREADY;
output  [C_M_AXI_SRC_ADDR_WIDTH - 1:0] m_axi_src_AWADDR;
output  [C_M_AXI_SRC_ID_WIDTH - 1:0] m_axi_src_AWID;
output  [7:0] m_axi_src_AWLEN;
output  [2:0] m_axi_src_AWSIZE;
output  [1:0] m_axi_src_AWBURST;
output  [1:0] m_axi_src_AWLOCK;
output  [3:0] m_axi_src_AWCACHE;
output  [2:0] m_axi_src_AWPROT;
output  [3:0] m_axi_src_AWQOS;
output  [3:0] m_axi_src_AWREGION;
output  [C_M_AXI_SRC_AWUSER_WIDTH - 1:0] m_axi_src_AWUSER;
output   m_axi_src_WVALID;
input   m_axi_src_WREADY;
output  [C_M_AXI_SRC_DATA_WIDTH - 1:0] m_axi_src_WDATA;
output  [C_M_AXI_SRC_WSTRB_WIDTH - 1:0] m_axi_src_WSTRB;
output   m_axi_src_WLAST;
output  [C_M_AXI_SRC_ID_WIDTH - 1:0] m_axi_src_WID;
output  [C_M_AXI_SRC_WUSER_WIDTH - 1:0] m_axi_src_WUSER;
output   m_axi_src_ARVALID;
input   m_axi_src_ARREADY;
output  [C_M_AXI_SRC_ADDR_WIDTH - 1:0] m_axi_src_ARADDR;
output  [C_M_AXI_SRC_ID_WIDTH - 1:0] m_axi_src_ARID;
output  [7:0] m_axi_src_ARLEN;
output  [2:0] m_axi_src_ARSIZE;
output  [1:0] m_axi_src_ARBURST;
output  [1:0] m_axi_src_ARLOCK;
output  [3:0] m_axi_src_ARCACHE;
output  [2:0] m_axi_src_ARPROT;
output  [3:0] m_axi_src_ARQOS;
output  [3:0] m_axi_src_ARREGION;
output  [C_M_AXI_SRC_ARUSER_WIDTH - 1:0] m_axi_src_ARUSER;
input   m_axi_src_RVALID;
output   m_axi_src_RREADY;
input  [C_M_AXI_SRC_DATA_WIDTH - 1:0] m_axi_src_RDATA;
input   m_axi_src_RLAST;
input  [C_M_AXI_SRC_ID_WIDTH - 1:0] m_axi_src_RID;
input  [C_M_AXI_SRC_RUSER_WIDTH - 1:0] m_axi_src_RUSER;
input  [1:0] m_axi_src_RRESP;
input   m_axi_src_BVALID;
output   m_axi_src_BREADY;
input  [1:0] m_axi_src_BRESP;
input  [C_M_AXI_SRC_ID_WIDTH - 1:0] m_axi_src_BID;
input  [C_M_AXI_SRC_BUSER_WIDTH - 1:0] m_axi_src_BUSER;
input  [63:0] src_r;
output  [32:0] out_s_din;
input   out_s_full_n;
output   out_s_write;
input  [32:0] out_peek;
input  [31:0] n;

assign ap_done = 'd0;
assign ap_idle = 'd0;
assign ap_ready = 'd0;
assign m_axi_src_AWVALID = 'd0;
assign m_axi_src_AWADDR = 'd0;
assign m_axi_src_AWID = 'd0;
assign m_axi_src_AWLEN = 'd0;
assign m_axi_src_AWSIZE = 'd0;
assign m_axi_src_AWBURST = 'd0;
assign m_axi_src_AWLOCK = 'd0;
assign m_axi_src_AWCACHE = 'd0;
assign m_axi_src_AWPROT = 'd0;
assign m_axi_src_AWQOS = 'd0;
assign m_axi_src_AWREGION = 'd0;
assign m_axi_src_AWUSER = 'd0;
assign m_axi_src_WVALID = 'd0;
assign m_axi_src_WDATA = 'd0;
assign m_axi_src_WSTRB = 'd0;
assign m_axi_src_WLAST = 'd0;
assign m_axi_src_WID = 'd0;
assign m_axi_src_WUSER = 'd0;
assign m_axi_src_ARVALID = 'd0;
assign m_axi_src_ARADDR = 'd0;
assign m_axi_src_ARID = 'd0;
assign m_axi_src_ARLEN = 'd0;
assign m_axi_src_ARSIZE = 'd0;
assign m_axi_src_ARBURST = 'd0;
assign m_axi_src_ARLOCK = 'd0;
assign m_axi_src_ARCACHE = 'd0;
assign m_axi_src_ARPROT = 'd0;
assign m_axi_src_ARQOS = 'd0;
assign m_axi_src_ARREGION = 'd0;
assign m_axi_src_ARUSER = 'd0;
assign m_axi_src_RREADY = 'd0;
assign m_axi_src_BREADY = 'd0;
assign out_s_din = 'd0;
assign out_s_write = 'd0;

endmodule
