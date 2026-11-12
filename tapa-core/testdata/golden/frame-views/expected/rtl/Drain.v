module Drain (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire [32:0] turned_s_dout,
  input wire turned_s_empty_n,
  output wire turned_s_read,
  input wire [32:0] turned_peek_dout,
  input wire turned_peek_empty_n,
  output wire turned_peek_read,
  input wire [63:0] dst_offset,
  input wire [31:0] n,
  output wire [63:0] m_axi_dst_ARADDR,
  output wire [1:0] m_axi_dst_ARBURST,
  output wire [3:0] m_axi_dst_ARCACHE,
  output wire m_axi_dst_ARID,
  output wire [7:0] m_axi_dst_ARLEN,
  output wire m_axi_dst_ARLOCK,
  output wire [2:0] m_axi_dst_ARPROT,
  output wire [3:0] m_axi_dst_ARQOS,
  input wire m_axi_dst_ARREADY,
  output wire [2:0] m_axi_dst_ARSIZE,
  output wire m_axi_dst_ARVALID,
  output wire [63:0] m_axi_dst_AWADDR,
  output wire [1:0] m_axi_dst_AWBURST,
  output wire [3:0] m_axi_dst_AWCACHE,
  output wire m_axi_dst_AWID,
  output wire [7:0] m_axi_dst_AWLEN,
  output wire m_axi_dst_AWLOCK,
  output wire [2:0] m_axi_dst_AWPROT,
  output wire [3:0] m_axi_dst_AWQOS,
  input wire m_axi_dst_AWREADY,
  output wire [2:0] m_axi_dst_AWSIZE,
  output wire m_axi_dst_AWVALID,
  input wire m_axi_dst_BID,
  output wire m_axi_dst_BREADY,
  input wire [1:0] m_axi_dst_BRESP,
  input wire m_axi_dst_BVALID,
  input wire [31:0] m_axi_dst_RDATA,
  input wire m_axi_dst_RID,
  input wire m_axi_dst_RLAST,
  output wire m_axi_dst_RREADY,
  input wire [1:0] m_axi_dst_RRESP,
  input wire m_axi_dst_RVALID,
  output wire [31:0] m_axi_dst_WDATA,
  output wire m_axi_dst_WLAST,
  input wire m_axi_dst_WREADY,
  output wire [3:0] m_axi_dst_WSTRB,
  output wire m_axi_dst_WVALID
);

wire ap_rst;
reg [1:0] Store_0__state;
wire Store_0__ap_start;
wire Store_0__ap_done;
wire Store_0__is_done;
wire Store_0__ap_idle;
wire Store_0__ap_ready;
wire [63:0] Store_0__dst_offset;
wire [31:0] Store_0__n;
reg [1:0] TurnIn_0__state;
wire TurnIn_0__ap_start;
wire TurnIn_0__ap_done;
wire TurnIn_0__is_done;
wire TurnIn_0__ap_idle;
wire TurnIn_0__ap_ready;
wire [31:0] TurnIn_0__n;
wire [17:0] cols_w_buf_address0;
wire cols_w_buf_ce0;
wire cols_w_buf_we0;
wire [31:0] cols_w_buf_d0;
wire [7:0] cols_w_acquire_dout;
wire cols_w_acquire_empty_n;
wire cols_w_acquire_read;
wire cols_w_release_din;
wire cols_w_release_full_n;
wire cols_w_release_write;
wire [32:0] cols_dout;
wire cols_empty_n;
wire cols_read;

Store Store_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Store_0__ap_start),
  .ap_done(Store_0__ap_done),
  .ap_idle(Store_0__ap_idle),
  .ap_ready(Store_0__ap_ready),
  .dst_r(Store_0__dst_offset),
  .m_axi_dst_ARADDR(m_axi_dst_ARADDR),
  .m_axi_dst_ARBURST(m_axi_dst_ARBURST),
  .m_axi_dst_ARID(m_axi_dst_ARID),
  .m_axi_dst_ARLEN(m_axi_dst_ARLEN),
  .m_axi_dst_ARREADY(m_axi_dst_ARREADY),
  .m_axi_dst_ARSIZE(m_axi_dst_ARSIZE),
  .m_axi_dst_ARVALID(m_axi_dst_ARVALID),
  .m_axi_dst_AWADDR(m_axi_dst_AWADDR),
  .m_axi_dst_AWBURST(m_axi_dst_AWBURST),
  .m_axi_dst_AWID(m_axi_dst_AWID),
  .m_axi_dst_AWLEN(m_axi_dst_AWLEN),
  .m_axi_dst_AWREADY(m_axi_dst_AWREADY),
  .m_axi_dst_AWSIZE(m_axi_dst_AWSIZE),
  .m_axi_dst_AWVALID(m_axi_dst_AWVALID),
  .m_axi_dst_BID(m_axi_dst_BID),
  .m_axi_dst_BREADY(m_axi_dst_BREADY),
  .m_axi_dst_BRESP(m_axi_dst_BRESP),
  .m_axi_dst_BVALID(m_axi_dst_BVALID),
  .m_axi_dst_RDATA(m_axi_dst_RDATA),
  .m_axi_dst_RID(m_axi_dst_RID),
  .m_axi_dst_RLAST(m_axi_dst_RLAST),
  .m_axi_dst_RREADY(m_axi_dst_RREADY),
  .m_axi_dst_RRESP(m_axi_dst_RRESP),
  .m_axi_dst_RVALID(m_axi_dst_RVALID),
  .m_axi_dst_WDATA(m_axi_dst_WDATA),
  .m_axi_dst_WLAST(m_axi_dst_WLAST),
  .m_axi_dst_WREADY(m_axi_dst_WREADY),
  .m_axi_dst_WSTRB(m_axi_dst_WSTRB),
  .m_axi_dst_WVALID(m_axi_dst_WVALID),
  .in_s_dout(cols_dout),
  .in_peek_dout(cols_dout),
  .in_s_empty_n(cols_empty_n),
  .in_peek_empty_n(cols_empty_n),
  .in_s_read(cols_read),
  .n(Store_0__n)
);

TurnIn TurnIn_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(TurnIn_0__ap_start),
  .ap_done(TurnIn_0__ap_done),
  .ap_idle(TurnIn_0__ap_idle),
  .ap_ready(TurnIn_0__ap_ready),
  .in_s_dout(turned_s_dout),
  .in_peek_dout(turned_peek_dout),
  .in_s_empty_n(turned_s_empty_n),
  .in_peek_empty_n(turned_peek_empty_n),
  .in_s_read(turned_s_read),
  .n(TurnIn_0__n),
  .out_buf_address0(cols_w_buf_address0),
  .out_buf_ce0(cols_w_buf_ce0),
  .out_buf_we0(cols_w_buf_we0),
  .out_buf_d0(cols_w_buf_d0),
  .out_acquire_dout(cols_w_acquire_dout),
  .out_acquire_empty_n(cols_w_acquire_empty_n),
  .out_acquire_read(cols_w_acquire_read),
  .out_release_din(cols_w_release_din),
  .out_release_full_n(cols_w_release_full_n),
  .out_release_write(cols_w_release_write)
);

Drain_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .Store_0__ap_start(Store_0__ap_start),
  .Store_0__ap_ready(Store_0__ap_ready),
  .Store_0__ap_done(Store_0__ap_done),
  .Store_0__ap_idle(Store_0__ap_idle),
  .Store_0__is_done(Store_0__is_done),
  .Store_0__dst_offset_in(dst_offset),
  .Store_0__dst_offset(Store_0__dst_offset),
  .Store_0__n_in(n),
  .Store_0__n(Store_0__n),
  .TurnIn_0__ap_start(TurnIn_0__ap_start),
  .TurnIn_0__ap_ready(TurnIn_0__ap_ready),
  .TurnIn_0__ap_done(TurnIn_0__ap_done),
  .TurnIn_0__ap_idle(TurnIn_0__ap_idle),
  .TurnIn_0__is_done(TurnIn_0__is_done),
  .TurnIn_0__n_in(n),
  .TurnIn_0__n(TurnIn_0__n)
);

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(1),
  .R_VIEW(0)
) cols_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0(cols_w_buf_address0),
  .w_buf_ce0(cols_w_buf_ce0),
  .w_buf_we0(cols_w_buf_we0),
  .w_buf_d0(cols_w_buf_d0),
  .w_acquire_dout(cols_w_acquire_dout),
  .w_acquire_empty_n(cols_w_acquire_empty_n),
  .w_acquire_read(cols_w_acquire_read),
  .w_release_full_n(cols_w_release_full_n),
  .w_release_write(cols_w_release_write),
  .if_din('d0),
  .if_write('d0),
  .r_buf_address0('d0),
  .r_buf_ce0('d0),
  .r_acquire_read('d0),
  .r_release_write('d0),
  .if_dout(cols_dout),
  .if_empty_n(cols_empty_n),
  .if_read(cols_read)
);

assign ap_rst = !ap_rst_n;
assign m_axi_dst_AWLOCK = 1'b0;
assign m_axi_dst_AWCACHE = 4'b0011;
assign m_axi_dst_AWPROT = 3'b000;
assign m_axi_dst_AWQOS = 4'b0000;
assign m_axi_dst_ARLOCK = 1'b0;
assign m_axi_dst_ARCACHE = 4'b0011;
assign m_axi_dst_ARPROT = 3'b000;
assign m_axi_dst_ARQOS = 4'b0000;
endmodule //Drain
