module CornerTurn #(
  parameter C_S_AXI_CONTROL_DATA_WIDTH = 32,
  parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6,
  parameter C_S_AXI_DATA_WIDTH = 32,
  parameter C_S_AXI_CONTROL_WSTRB_WIDTH = (32/8),
  parameter C_S_AXI_WSTRB_WIDTH = (32/8)
) (
  input wire s_axi_control_AWVALID,
  output wire s_axi_control_AWREADY,
  input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_AWADDR,
  input wire s_axi_control_WVALID,
  output wire s_axi_control_WREADY,
  input wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_WDATA,
  input wire [C_S_AXI_CONTROL_WSTRB_WIDTH-1:0] s_axi_control_WSTRB,
  input wire s_axi_control_ARVALID,
  output wire s_axi_control_ARREADY,
  input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_ARADDR,
  output wire s_axi_control_RVALID,
  input wire s_axi_control_RREADY,
  output wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_RDATA,
  output wire [1:0] s_axi_control_RRESP,
  output wire s_axi_control_BVALID,
  input wire s_axi_control_BREADY,
  output wire [1:0] s_axi_control_BRESP,
  input wire ap_clk,
  input wire ap_rst_n,
  output wire interrupt,
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
  output wire m_axi_dst_WVALID,
  output wire [63:0] m_axi_src_ARADDR,
  output wire [1:0] m_axi_src_ARBURST,
  output wire [3:0] m_axi_src_ARCACHE,
  output wire m_axi_src_ARID,
  output wire [7:0] m_axi_src_ARLEN,
  output wire m_axi_src_ARLOCK,
  output wire [2:0] m_axi_src_ARPROT,
  output wire [3:0] m_axi_src_ARQOS,
  input wire m_axi_src_ARREADY,
  output wire [2:0] m_axi_src_ARSIZE,
  output wire m_axi_src_ARVALID,
  output wire [63:0] m_axi_src_AWADDR,
  output wire [1:0] m_axi_src_AWBURST,
  output wire [3:0] m_axi_src_AWCACHE,
  output wire m_axi_src_AWID,
  output wire [7:0] m_axi_src_AWLEN,
  output wire m_axi_src_AWLOCK,
  output wire [2:0] m_axi_src_AWPROT,
  output wire [3:0] m_axi_src_AWQOS,
  input wire m_axi_src_AWREADY,
  output wire [2:0] m_axi_src_AWSIZE,
  output wire m_axi_src_AWVALID,
  input wire m_axi_src_BID,
  output wire m_axi_src_BREADY,
  input wire [1:0] m_axi_src_BRESP,
  input wire m_axi_src_BVALID,
  input wire [31:0] m_axi_src_RDATA,
  input wire m_axi_src_RID,
  input wire m_axi_src_RLAST,
  output wire m_axi_src_RREADY,
  input wire [1:0] m_axi_src_RRESP,
  input wire m_axi_src_RVALID,
  output wire [31:0] m_axi_src_WDATA,
  output wire m_axi_src_WLAST,
  input wire m_axi_src_WREADY,
  output wire [3:0] m_axi_src_WSTRB,
  output wire m_axi_src_WVALID
);

wire ap_rst;
reg [1:0] Load_0__state;
wire Load_0__ap_start;
wire Load_0__ap_done;
wire Load_0__is_done;
wire Load_0__ap_idle;
wire Load_0__ap_ready;
wire [31:0] Load_0__n;
wire [63:0] Load_0__src_offset;
reg [1:0] Store_0__state;
wire Store_0__ap_start;
wire Store_0__ap_done;
wire Store_0__is_done;
wire Store_0__ap_idle;
wire Store_0__ap_ready;
wire [63:0] Store_0__dst_offset;
wire [31:0] Store_0__n;
reg [1:0] Turn_0__state;
wire Turn_0__ap_start;
wire Turn_0__ap_done;
wire Turn_0__is_done;
wire Turn_0__ap_idle;
wire Turn_0__ap_ready;
wire [31:0] Turn_0__n;
reg [1:0] Turn_1__state;
wire Turn_1__ap_start;
wire Turn_1__ap_done;
wire Turn_1__is_done;
wire Turn_1__ap_idle;
wire Turn_1__ap_ready;
wire [31:0] Turn_1__n;
reg [1:0] Turn_2__state;
wire Turn_2__ap_start;
wire Turn_2__ap_done;
wire Turn_2__is_done;
wire Turn_2__ap_idle;
wire Turn_2__ap_ready;
wire [31:0] Turn_2__n;
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
wire [17:0] once_w_buf_address0;
wire once_w_buf_ce0;
wire once_w_buf_we0;
wire [31:0] once_w_buf_d0;
wire [7:0] once_w_acquire_dout;
wire once_w_acquire_empty_n;
wire once_w_acquire_read;
wire once_w_release_din;
wire once_w_release_full_n;
wire once_w_release_write;
wire [17:0] once_r_buf_address0;
wire once_r_buf_ce0;
wire [31:0] once_r_buf_q0;
wire [7:0] once_r_acquire_dout;
wire once_r_acquire_empty_n;
wire once_r_acquire_read;
wire once_r_release_din;
wire once_r_release_full_n;
wire once_r_release_write;
wire [32:0] rows_din;
wire rows_full_n;
wire rows_write;
wire [17:0] rows_r_buf_address0;
wire rows_r_buf_ce0;
wire [31:0] rows_r_buf_q0;
wire [7:0] rows_r_acquire_dout;
wire rows_r_acquire_empty_n;
wire rows_r_acquire_read;
wire rows_r_release_din;
wire rows_r_release_full_n;
wire rows_r_release_write;
wire [17:0] twice_w_buf_address0;
wire twice_w_buf_ce0;
wire twice_w_buf_we0;
wire [31:0] twice_w_buf_d0;
wire [7:0] twice_w_acquire_dout;
wire twice_w_acquire_empty_n;
wire twice_w_acquire_read;
wire twice_w_release_din;
wire twice_w_release_full_n;
wire twice_w_release_write;
wire [17:0] twice_r_buf_address0;
wire twice_r_buf_ce0;
wire [31:0] twice_r_buf_q0;
wire [7:0] twice_r_acquire_dout;
wire twice_r_acquire_empty_n;
wire twice_r_acquire_read;
wire twice_r_release_din;
wire twice_r_release_full_n;
wire twice_r_release_write;
wire [63:0] src_offset;
wire [63:0] dst_offset;
wire [31:0] n;

Load Load_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Load_0__ap_start),
  .ap_done(Load_0__ap_done),
  .ap_idle(Load_0__ap_idle),
  .ap_ready(Load_0__ap_ready),
  .n(Load_0__n),
  .out_s_din(rows_din),
  .out_s_full_n(rows_full_n),
  .out_s_write(rows_write),
  .out_peek('d0),
  .src_r(Load_0__src_offset),
  .m_axi_src_ARADDR(m_axi_src_ARADDR),
  .m_axi_src_ARBURST(m_axi_src_ARBURST),
  .m_axi_src_ARID(m_axi_src_ARID),
  .m_axi_src_ARLEN(m_axi_src_ARLEN),
  .m_axi_src_ARREADY(m_axi_src_ARREADY),
  .m_axi_src_ARSIZE(m_axi_src_ARSIZE),
  .m_axi_src_ARVALID(m_axi_src_ARVALID),
  .m_axi_src_AWADDR(m_axi_src_AWADDR),
  .m_axi_src_AWBURST(m_axi_src_AWBURST),
  .m_axi_src_AWID(m_axi_src_AWID),
  .m_axi_src_AWLEN(m_axi_src_AWLEN),
  .m_axi_src_AWREADY(m_axi_src_AWREADY),
  .m_axi_src_AWSIZE(m_axi_src_AWSIZE),
  .m_axi_src_AWVALID(m_axi_src_AWVALID),
  .m_axi_src_BID(m_axi_src_BID),
  .m_axi_src_BREADY(m_axi_src_BREADY),
  .m_axi_src_BRESP(m_axi_src_BRESP),
  .m_axi_src_BVALID(m_axi_src_BVALID),
  .m_axi_src_RDATA(m_axi_src_RDATA),
  .m_axi_src_RID(m_axi_src_RID),
  .m_axi_src_RLAST(m_axi_src_RLAST),
  .m_axi_src_RREADY(m_axi_src_RREADY),
  .m_axi_src_RRESP(m_axi_src_RRESP),
  .m_axi_src_RVALID(m_axi_src_RVALID),
  .m_axi_src_WDATA(m_axi_src_WDATA),
  .m_axi_src_WLAST(m_axi_src_WLAST),
  .m_axi_src_WREADY(m_axi_src_WREADY),
  .m_axi_src_WSTRB(m_axi_src_WSTRB),
  .m_axi_src_WVALID(m_axi_src_WVALID)
);

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

Turn Turn_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Turn_0__ap_start),
  .ap_done(Turn_0__ap_done),
  .ap_idle(Turn_0__ap_idle),
  .ap_ready(Turn_0__ap_ready),
  .in_buf_address0(rows_r_buf_address0),
  .in_buf_ce0(rows_r_buf_ce0),
  .in_buf_q0(rows_r_buf_q0),
  .in_acquire_dout(rows_r_acquire_dout),
  .in_acquire_empty_n(rows_r_acquire_empty_n),
  .in_acquire_read(rows_r_acquire_read),
  .in_release_din(rows_r_release_din),
  .in_release_full_n(rows_r_release_full_n),
  .in_release_write(rows_r_release_write),
  .n(Turn_0__n),
  .out_buf_address0(once_w_buf_address0),
  .out_buf_ce0(once_w_buf_ce0),
  .out_buf_we0(once_w_buf_we0),
  .out_buf_d0(once_w_buf_d0),
  .out_acquire_dout(once_w_acquire_dout),
  .out_acquire_empty_n(once_w_acquire_empty_n),
  .out_acquire_read(once_w_acquire_read),
  .out_release_din(once_w_release_din),
  .out_release_full_n(once_w_release_full_n),
  .out_release_write(once_w_release_write)
);

Turn Turn_1 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Turn_1__ap_start),
  .ap_done(Turn_1__ap_done),
  .ap_idle(Turn_1__ap_idle),
  .ap_ready(Turn_1__ap_ready),
  .in_buf_address0(once_r_buf_address0),
  .in_buf_ce0(once_r_buf_ce0),
  .in_buf_q0(once_r_buf_q0),
  .in_acquire_dout(once_r_acquire_dout),
  .in_acquire_empty_n(once_r_acquire_empty_n),
  .in_acquire_read(once_r_acquire_read),
  .in_release_din(once_r_release_din),
  .in_release_full_n(once_r_release_full_n),
  .in_release_write(once_r_release_write),
  .n(Turn_1__n),
  .out_buf_address0(twice_w_buf_address0),
  .out_buf_ce0(twice_w_buf_ce0),
  .out_buf_we0(twice_w_buf_we0),
  .out_buf_d0(twice_w_buf_d0),
  .out_acquire_dout(twice_w_acquire_dout),
  .out_acquire_empty_n(twice_w_acquire_empty_n),
  .out_acquire_read(twice_w_acquire_read),
  .out_release_din(twice_w_release_din),
  .out_release_full_n(twice_w_release_full_n),
  .out_release_write(twice_w_release_write)
);

Turn Turn_2 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Turn_2__ap_start),
  .ap_done(Turn_2__ap_done),
  .ap_idle(Turn_2__ap_idle),
  .ap_ready(Turn_2__ap_ready),
  .in_buf_address0(twice_r_buf_address0),
  .in_buf_ce0(twice_r_buf_ce0),
  .in_buf_q0(twice_r_buf_q0),
  .in_acquire_dout(twice_r_acquire_dout),
  .in_acquire_empty_n(twice_r_acquire_empty_n),
  .in_acquire_read(twice_r_acquire_read),
  .in_release_din(twice_r_release_din),
  .in_release_full_n(twice_r_release_full_n),
  .in_release_write(twice_r_release_write),
  .n(Turn_2__n),
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

CornerTurn_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .Load_0__ap_start(Load_0__ap_start),
  .Load_0__ap_ready(Load_0__ap_ready),
  .Load_0__ap_done(Load_0__ap_done),
  .Load_0__ap_idle(Load_0__ap_idle),
  .Load_0__is_done(Load_0__is_done),
  .Load_0__n_in(n),
  .Load_0__n(Load_0__n),
  .Load_0__src_offset_in(src_offset),
  .Load_0__src_offset(Load_0__src_offset),
  .Store_0__ap_start(Store_0__ap_start),
  .Store_0__ap_ready(Store_0__ap_ready),
  .Store_0__ap_done(Store_0__ap_done),
  .Store_0__ap_idle(Store_0__ap_idle),
  .Store_0__is_done(Store_0__is_done),
  .Store_0__dst_offset_in(dst_offset),
  .Store_0__dst_offset(Store_0__dst_offset),
  .Store_0__n_in(n),
  .Store_0__n(Store_0__n),
  .Turn_0__ap_start(Turn_0__ap_start),
  .Turn_0__ap_ready(Turn_0__ap_ready),
  .Turn_0__ap_done(Turn_0__ap_done),
  .Turn_0__ap_idle(Turn_0__ap_idle),
  .Turn_0__is_done(Turn_0__is_done),
  .Turn_0__n_in(n),
  .Turn_0__n(Turn_0__n),
  .Turn_1__ap_start(Turn_1__ap_start),
  .Turn_1__ap_ready(Turn_1__ap_ready),
  .Turn_1__ap_done(Turn_1__ap_done),
  .Turn_1__ap_idle(Turn_1__ap_idle),
  .Turn_1__is_done(Turn_1__is_done),
  .Turn_1__n_in(n),
  .Turn_1__n(Turn_1__n),
  .Turn_2__ap_start(Turn_2__ap_start),
  .Turn_2__ap_ready(Turn_2__ap_ready),
  .Turn_2__ap_done(Turn_2__ap_done),
  .Turn_2__ap_idle(Turn_2__ap_idle),
  .Turn_2__is_done(Turn_2__is_done),
  .Turn_2__n_in(n),
  .Turn_2__n(Turn_2__n)
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

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(1),
  .R_VIEW(1)
) once_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0(once_w_buf_address0),
  .w_buf_ce0(once_w_buf_ce0),
  .w_buf_we0(once_w_buf_we0),
  .w_buf_d0(once_w_buf_d0),
  .w_acquire_dout(once_w_acquire_dout),
  .w_acquire_empty_n(once_w_acquire_empty_n),
  .w_acquire_read(once_w_acquire_read),
  .w_release_full_n(once_w_release_full_n),
  .w_release_write(once_w_release_write),
  .if_din('d0),
  .if_write('d0),
  .r_buf_address0(once_r_buf_address0),
  .r_buf_ce0(once_r_buf_ce0),
  .r_buf_q0(once_r_buf_q0),
  .r_acquire_dout(once_r_acquire_dout),
  .r_acquire_empty_n(once_r_acquire_empty_n),
  .r_acquire_read(once_r_acquire_read),
  .r_release_full_n(once_r_release_full_n),
  .r_release_write(once_r_release_write),
  .if_read('d0)
);

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(0),
  .R_VIEW(1)
) rows_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0('d0),
  .w_buf_ce0('d0),
  .w_buf_we0('d0),
  .w_buf_d0('d0),
  .w_acquire_read('d0),
  .w_release_write('d0),
  .if_din(rows_din),
  .if_full_n(rows_full_n),
  .if_write(rows_write),
  .r_buf_address0(rows_r_buf_address0),
  .r_buf_ce0(rows_r_buf_ce0),
  .r_buf_q0(rows_r_buf_q0),
  .r_acquire_dout(rows_r_acquire_dout),
  .r_acquire_empty_n(rows_r_acquire_empty_n),
  .r_acquire_read(rows_r_acquire_read),
  .r_release_full_n(rows_r_release_full_n),
  .r_release_write(rows_r_release_write),
  .if_read('d0)
);

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(1),
  .R_VIEW(1)
) twice_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0(twice_w_buf_address0),
  .w_buf_ce0(twice_w_buf_ce0),
  .w_buf_we0(twice_w_buf_we0),
  .w_buf_d0(twice_w_buf_d0),
  .w_acquire_dout(twice_w_acquire_dout),
  .w_acquire_empty_n(twice_w_acquire_empty_n),
  .w_acquire_read(twice_w_acquire_read),
  .w_release_full_n(twice_w_release_full_n),
  .w_release_write(twice_w_release_write),
  .if_din('d0),
  .if_write('d0),
  .r_buf_address0(twice_r_buf_address0),
  .r_buf_ce0(twice_r_buf_ce0),
  .r_buf_q0(twice_r_buf_q0),
  .r_acquire_dout(twice_r_acquire_dout),
  .r_acquire_empty_n(twice_r_acquire_empty_n),
  .r_acquire_read(twice_r_acquire_read),
  .r_release_full_n(twice_r_release_full_n),
  .r_release_write(twice_r_release_write),
  .if_read('d0)
);

CornerTurn_control_s_axi #(
  .C_S_AXI_ADDR_WIDTH(C_S_AXI_CONTROL_ADDR_WIDTH),
  .C_S_AXI_DATA_WIDTH(C_S_AXI_CONTROL_DATA_WIDTH)
) control_s_axi_U (
  .ACLK(ap_clk),
  .ARESET(ap_rst),
  .ACLK_EN(1'b1),
  .AWVALID(s_axi_control_AWVALID),
  .AWREADY(s_axi_control_AWREADY),
  .AWADDR(s_axi_control_AWADDR),
  .WVALID(s_axi_control_WVALID),
  .WREADY(s_axi_control_WREADY),
  .WDATA(s_axi_control_WDATA),
  .WSTRB(s_axi_control_WSTRB),
  .ARVALID(s_axi_control_ARVALID),
  .ARREADY(s_axi_control_ARREADY),
  .ARADDR(s_axi_control_ARADDR),
  .RVALID(s_axi_control_RVALID),
  .RREADY(s_axi_control_RREADY),
  .RDATA(s_axi_control_RDATA),
  .RRESP(s_axi_control_RRESP),
  .BVALID(s_axi_control_BVALID),
  .BREADY(s_axi_control_BREADY),
  .BRESP(s_axi_control_BRESP),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .interrupt(interrupt),
  .src_offset(src_offset),
  .dst_offset(dst_offset),
  .n(n)
);

assign ap_rst = !ap_rst_n;
endmodule //CornerTurn
