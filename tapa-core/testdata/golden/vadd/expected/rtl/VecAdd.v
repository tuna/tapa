module VecAdd #(
  parameter C_S_AXI_CONTROL_DATA_WIDTH = 32,
  parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6
) (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire s_axi_control_AWVALID,
  output wire s_axi_control_AWREADY,
  input wire [5:0] s_axi_control_AWADDR,
  input wire s_axi_control_WVALID,
  output wire s_axi_control_WREADY,
  input wire [31:0] s_axi_control_WDATA,
  input wire [3:0] s_axi_control_WSTRB,
  input wire s_axi_control_ARVALID,
  output wire s_axi_control_ARREADY,
  input wire [5:0] s_axi_control_ARADDR,
  output wire s_axi_control_RVALID,
  input wire s_axi_control_RREADY,
  output wire [31:0] s_axi_control_RDATA,
  output wire [1:0] s_axi_control_RRESP,
  output wire s_axi_control_BVALID,
  input wire s_axi_control_BREADY,
  output wire [1:0] s_axi_control_BRESP,
  output wire interrupt,
  output wire [63:0] m_axi_a_ARADDR,
  output wire [1:0] m_axi_a_ARBURST,
  output wire [3:0] m_axi_a_ARCACHE,
  output wire m_axi_a_ARID,
  output wire [7:0] m_axi_a_ARLEN,
  output wire m_axi_a_ARLOCK,
  output wire [2:0] m_axi_a_ARPROT,
  output wire [3:0] m_axi_a_ARQOS,
  input wire m_axi_a_ARREADY,
  output wire [2:0] m_axi_a_ARSIZE,
  output wire m_axi_a_ARVALID,
  output wire [63:0] m_axi_a_AWADDR,
  output wire [1:0] m_axi_a_AWBURST,
  output wire [3:0] m_axi_a_AWCACHE,
  output wire m_axi_a_AWID,
  output wire [7:0] m_axi_a_AWLEN,
  output wire m_axi_a_AWLOCK,
  output wire [2:0] m_axi_a_AWPROT,
  output wire [3:0] m_axi_a_AWQOS,
  input wire m_axi_a_AWREADY,
  output wire [2:0] m_axi_a_AWSIZE,
  output wire m_axi_a_AWVALID,
  input wire m_axi_a_BID,
  output wire m_axi_a_BREADY,
  input wire [1:0] m_axi_a_BRESP,
  input wire m_axi_a_BVALID,
  input wire [31:0] m_axi_a_RDATA,
  input wire m_axi_a_RID,
  input wire m_axi_a_RLAST,
  output wire m_axi_a_RREADY,
  input wire [1:0] m_axi_a_RRESP,
  input wire m_axi_a_RVALID,
  output wire [31:0] m_axi_a_WDATA,
  output wire m_axi_a_WLAST,
  input wire m_axi_a_WREADY,
  output wire [3:0] m_axi_a_WSTRB,
  output wire m_axi_a_WVALID,
  output wire [63:0] m_axi_b_ARADDR,
  output wire [1:0] m_axi_b_ARBURST,
  output wire [3:0] m_axi_b_ARCACHE,
  output wire m_axi_b_ARID,
  output wire [7:0] m_axi_b_ARLEN,
  output wire m_axi_b_ARLOCK,
  output wire [2:0] m_axi_b_ARPROT,
  output wire [3:0] m_axi_b_ARQOS,
  input wire m_axi_b_ARREADY,
  output wire [2:0] m_axi_b_ARSIZE,
  output wire m_axi_b_ARVALID,
  output wire [63:0] m_axi_b_AWADDR,
  output wire [1:0] m_axi_b_AWBURST,
  output wire [3:0] m_axi_b_AWCACHE,
  output wire m_axi_b_AWID,
  output wire [7:0] m_axi_b_AWLEN,
  output wire m_axi_b_AWLOCK,
  output wire [2:0] m_axi_b_AWPROT,
  output wire [3:0] m_axi_b_AWQOS,
  input wire m_axi_b_AWREADY,
  output wire [2:0] m_axi_b_AWSIZE,
  output wire m_axi_b_AWVALID,
  input wire m_axi_b_BID,
  output wire m_axi_b_BREADY,
  input wire [1:0] m_axi_b_BRESP,
  input wire m_axi_b_BVALID,
  input wire [31:0] m_axi_b_RDATA,
  input wire m_axi_b_RID,
  input wire m_axi_b_RLAST,
  output wire m_axi_b_RREADY,
  input wire [1:0] m_axi_b_RRESP,
  input wire m_axi_b_RVALID,
  output wire [31:0] m_axi_b_WDATA,
  output wire m_axi_b_WLAST,
  input wire m_axi_b_WREADY,
  output wire [3:0] m_axi_b_WSTRB,
  output wire m_axi_b_WVALID,
  output wire [63:0] m_axi_c_ARADDR,
  output wire [1:0] m_axi_c_ARBURST,
  output wire [3:0] m_axi_c_ARCACHE,
  output wire m_axi_c_ARID,
  output wire [7:0] m_axi_c_ARLEN,
  output wire m_axi_c_ARLOCK,
  output wire [2:0] m_axi_c_ARPROT,
  output wire [3:0] m_axi_c_ARQOS,
  input wire m_axi_c_ARREADY,
  output wire [2:0] m_axi_c_ARSIZE,
  output wire m_axi_c_ARVALID,
  output wire [63:0] m_axi_c_AWADDR,
  output wire [1:0] m_axi_c_AWBURST,
  output wire [3:0] m_axi_c_AWCACHE,
  output wire m_axi_c_AWID,
  output wire [7:0] m_axi_c_AWLEN,
  output wire m_axi_c_AWLOCK,
  output wire [2:0] m_axi_c_AWPROT,
  output wire [3:0] m_axi_c_AWQOS,
  input wire m_axi_c_AWREADY,
  output wire [2:0] m_axi_c_AWSIZE,
  output wire m_axi_c_AWVALID,
  input wire m_axi_c_BID,
  output wire m_axi_c_BREADY,
  input wire [1:0] m_axi_c_BRESP,
  input wire m_axi_c_BVALID,
  input wire [31:0] m_axi_c_RDATA,
  input wire m_axi_c_RID,
  input wire m_axi_c_RLAST,
  output wire m_axi_c_RREADY,
  input wire [1:0] m_axi_c_RRESP,
  input wire m_axi_c_RVALID,
  output wire [31:0] m_axi_c_WDATA,
  output wire m_axi_c_WLAST,
  input wire m_axi_c_WREADY,
  output wire [3:0] m_axi_c_WSTRB,
  output wire m_axi_c_WVALID
);

wire ap_rst;
reg [1:0] Add_0__state;
wire Add_0__ap_start;
wire Add_0__ap_done;
wire Add_0__is_done;
wire Add_0__ap_idle;
wire Add_0__ap_ready;
wire [63:0] Add_0__n;
reg [1:0] Mmap2Stream_0__state;
wire Mmap2Stream_0__ap_start;
wire Mmap2Stream_0__ap_done;
wire Mmap2Stream_0__is_done;
wire Mmap2Stream_0__ap_idle;
wire Mmap2Stream_0__ap_ready;
wire [63:0] Mmap2Stream_0__mmap_port_offset;
reg [1:0] Mmap2Stream_1__state;
wire Mmap2Stream_1__ap_start;
wire Mmap2Stream_1__ap_done;
wire Mmap2Stream_1__is_done;
wire Mmap2Stream_1__ap_idle;
wire Mmap2Stream_1__ap_ready;
wire [63:0] Mmap2Stream_1__mmap_port_offset;
reg [1:0] Stream2Mmap_0__state;
wire Stream2Mmap_0__ap_start;
wire Stream2Mmap_0__ap_done;
wire Stream2Mmap_0__is_done;
wire Stream2Mmap_0__ap_idle;
wire Stream2Mmap_0__ap_ready;
wire [63:0] Stream2Mmap_0__mmap_port_offset;
wire [32:0] a_q_dout;
wire a_q_empty_n;
wire a_q_read;
wire [32:0] a_q_din;
wire a_q_full_n;
wire a_q_write;
wire [32:0] b_q_dout;
wire b_q_empty_n;
wire b_q_read;
wire [32:0] b_q_din;
wire b_q_full_n;
wire b_q_write;
wire [32:0] c_q_dout;
wire c_q_empty_n;
wire c_q_read;
wire [32:0] c_q_din;
wire c_q_full_n;
wire c_q_write;
wire [63:0] a_offset;
wire [63:0] b_offset;
wire [63:0] c_offset;
wire [63:0] n;

Add Add_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Add_0__ap_start),
  .ap_done(Add_0__ap_done),
  .ap_idle(Add_0__ap_idle),
  .ap_ready(Add_0__ap_ready),
  .a_dout(a_q_dout),
  .a_peek_dout(a_q_dout),
  .a_empty_n(a_q_empty_n),
  .a_peek_empty_n(a_q_empty_n),
  .a_read(a_q_read),
  .b_dout(b_q_dout),
  .b_peek_dout(b_q_dout),
  .b_empty_n(b_q_empty_n),
  .b_peek_empty_n(b_q_empty_n),
  .b_read(b_q_read),
  .c_din(c_q_din),
  .c_full_n(c_q_full_n),
  .c_write(c_q_write),
  .n(Add_0__n)
);

Mmap2Stream Mmap2Stream_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Mmap2Stream_0__ap_start),
  .ap_done(Mmap2Stream_0__ap_done),
  .ap_idle(Mmap2Stream_0__ap_idle),
  .ap_ready(Mmap2Stream_0__ap_ready),
  .mmap_port_offset(Mmap2Stream_0__mmap_port_offset),
  .m_axi_mmap_port_ARADDR(m_axi_a_ARADDR),
  .m_axi_mmap_port_ARBURST(m_axi_a_ARBURST),
  .m_axi_mmap_port_ARID(m_axi_a_ARID),
  .m_axi_mmap_port_ARLEN(m_axi_a_ARLEN),
  .m_axi_mmap_port_ARREADY(m_axi_a_ARREADY),
  .m_axi_mmap_port_ARSIZE(m_axi_a_ARSIZE),
  .m_axi_mmap_port_ARVALID(m_axi_a_ARVALID),
  .m_axi_mmap_port_AWADDR(m_axi_a_AWADDR),
  .m_axi_mmap_port_AWBURST(m_axi_a_AWBURST),
  .m_axi_mmap_port_AWID(m_axi_a_AWID),
  .m_axi_mmap_port_AWLEN(m_axi_a_AWLEN),
  .m_axi_mmap_port_AWREADY(m_axi_a_AWREADY),
  .m_axi_mmap_port_AWSIZE(m_axi_a_AWSIZE),
  .m_axi_mmap_port_AWVALID(m_axi_a_AWVALID),
  .m_axi_mmap_port_BID(m_axi_a_BID),
  .m_axi_mmap_port_BREADY(m_axi_a_BREADY),
  .m_axi_mmap_port_BRESP(m_axi_a_BRESP),
  .m_axi_mmap_port_BVALID(m_axi_a_BVALID),
  .m_axi_mmap_port_RDATA(m_axi_a_RDATA),
  .m_axi_mmap_port_RID(m_axi_a_RID),
  .m_axi_mmap_port_RLAST(m_axi_a_RLAST),
  .m_axi_mmap_port_RREADY(m_axi_a_RREADY),
  .m_axi_mmap_port_RRESP(m_axi_a_RRESP),
  .m_axi_mmap_port_RVALID(m_axi_a_RVALID),
  .m_axi_mmap_port_WDATA(m_axi_a_WDATA),
  .m_axi_mmap_port_WLAST(m_axi_a_WLAST),
  .m_axi_mmap_port_WREADY(m_axi_a_WREADY),
  .m_axi_mmap_port_WSTRB(m_axi_a_WSTRB),
  .m_axi_mmap_port_WVALID(m_axi_a_WVALID),
  .stream_port_din(a_q_din),
  .stream_port_full_n(a_q_full_n),
  .stream_port_write(a_q_write)
);

Mmap2Stream Mmap2Stream_1 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Mmap2Stream_1__ap_start),
  .ap_done(Mmap2Stream_1__ap_done),
  .ap_idle(Mmap2Stream_1__ap_idle),
  .ap_ready(Mmap2Stream_1__ap_ready),
  .mmap_port_offset(Mmap2Stream_1__mmap_port_offset),
  .m_axi_mmap_port_ARADDR(m_axi_b_ARADDR),
  .m_axi_mmap_port_ARBURST(m_axi_b_ARBURST),
  .m_axi_mmap_port_ARID(m_axi_b_ARID),
  .m_axi_mmap_port_ARLEN(m_axi_b_ARLEN),
  .m_axi_mmap_port_ARREADY(m_axi_b_ARREADY),
  .m_axi_mmap_port_ARSIZE(m_axi_b_ARSIZE),
  .m_axi_mmap_port_ARVALID(m_axi_b_ARVALID),
  .m_axi_mmap_port_AWADDR(m_axi_b_AWADDR),
  .m_axi_mmap_port_AWBURST(m_axi_b_AWBURST),
  .m_axi_mmap_port_AWID(m_axi_b_AWID),
  .m_axi_mmap_port_AWLEN(m_axi_b_AWLEN),
  .m_axi_mmap_port_AWREADY(m_axi_b_AWREADY),
  .m_axi_mmap_port_AWSIZE(m_axi_b_AWSIZE),
  .m_axi_mmap_port_AWVALID(m_axi_b_AWVALID),
  .m_axi_mmap_port_BID(m_axi_b_BID),
  .m_axi_mmap_port_BREADY(m_axi_b_BREADY),
  .m_axi_mmap_port_BRESP(m_axi_b_BRESP),
  .m_axi_mmap_port_BVALID(m_axi_b_BVALID),
  .m_axi_mmap_port_RDATA(m_axi_b_RDATA),
  .m_axi_mmap_port_RID(m_axi_b_RID),
  .m_axi_mmap_port_RLAST(m_axi_b_RLAST),
  .m_axi_mmap_port_RREADY(m_axi_b_RREADY),
  .m_axi_mmap_port_RRESP(m_axi_b_RRESP),
  .m_axi_mmap_port_RVALID(m_axi_b_RVALID),
  .m_axi_mmap_port_WDATA(m_axi_b_WDATA),
  .m_axi_mmap_port_WLAST(m_axi_b_WLAST),
  .m_axi_mmap_port_WREADY(m_axi_b_WREADY),
  .m_axi_mmap_port_WSTRB(m_axi_b_WSTRB),
  .m_axi_mmap_port_WVALID(m_axi_b_WVALID),
  .stream_port_din(b_q_din),
  .stream_port_full_n(b_q_full_n),
  .stream_port_write(b_q_write)
);

Stream2Mmap Stream2Mmap_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(Stream2Mmap_0__ap_start),
  .ap_done(Stream2Mmap_0__ap_done),
  .ap_idle(Stream2Mmap_0__ap_idle),
  .ap_ready(Stream2Mmap_0__ap_ready),
  .mmap_port_offset(Stream2Mmap_0__mmap_port_offset),
  .m_axi_mmap_port_ARADDR(m_axi_c_ARADDR),
  .m_axi_mmap_port_ARBURST(m_axi_c_ARBURST),
  .m_axi_mmap_port_ARID(m_axi_c_ARID),
  .m_axi_mmap_port_ARLEN(m_axi_c_ARLEN),
  .m_axi_mmap_port_ARREADY(m_axi_c_ARREADY),
  .m_axi_mmap_port_ARSIZE(m_axi_c_ARSIZE),
  .m_axi_mmap_port_ARVALID(m_axi_c_ARVALID),
  .m_axi_mmap_port_AWADDR(m_axi_c_AWADDR),
  .m_axi_mmap_port_AWBURST(m_axi_c_AWBURST),
  .m_axi_mmap_port_AWID(m_axi_c_AWID),
  .m_axi_mmap_port_AWLEN(m_axi_c_AWLEN),
  .m_axi_mmap_port_AWREADY(m_axi_c_AWREADY),
  .m_axi_mmap_port_AWSIZE(m_axi_c_AWSIZE),
  .m_axi_mmap_port_AWVALID(m_axi_c_AWVALID),
  .m_axi_mmap_port_BID(m_axi_c_BID),
  .m_axi_mmap_port_BREADY(m_axi_c_BREADY),
  .m_axi_mmap_port_BRESP(m_axi_c_BRESP),
  .m_axi_mmap_port_BVALID(m_axi_c_BVALID),
  .m_axi_mmap_port_RDATA(m_axi_c_RDATA),
  .m_axi_mmap_port_RID(m_axi_c_RID),
  .m_axi_mmap_port_RLAST(m_axi_c_RLAST),
  .m_axi_mmap_port_RREADY(m_axi_c_RREADY),
  .m_axi_mmap_port_RRESP(m_axi_c_RRESP),
  .m_axi_mmap_port_RVALID(m_axi_c_RVALID),
  .m_axi_mmap_port_WDATA(m_axi_c_WDATA),
  .m_axi_mmap_port_WLAST(m_axi_c_WLAST),
  .m_axi_mmap_port_WREADY(m_axi_c_WREADY),
  .m_axi_mmap_port_WSTRB(m_axi_c_WSTRB),
  .m_axi_mmap_port_WVALID(m_axi_c_WVALID),
  .stream_port_dout(c_q_dout),
  .stream_port_peek_dout(c_q_dout),
  .stream_port_empty_n(c_q_empty_n),
  .stream_port_peek_empty_n(c_q_empty_n),
  .stream_port_read(c_q_read)
);

VecAdd_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .Add_0__ap_start(Add_0__ap_start),
  .Add_0__ap_ready(Add_0__ap_ready),
  .Add_0__ap_done(Add_0__ap_done),
  .Add_0__ap_idle(Add_0__ap_idle),
  .Add_0__is_done(Add_0__is_done),
  .Add_0__n_in(n),
  .Add_0__n(Add_0__n),
  .Mmap2Stream_0__ap_start(Mmap2Stream_0__ap_start),
  .Mmap2Stream_0__ap_ready(Mmap2Stream_0__ap_ready),
  .Mmap2Stream_0__ap_done(Mmap2Stream_0__ap_done),
  .Mmap2Stream_0__ap_idle(Mmap2Stream_0__ap_idle),
  .Mmap2Stream_0__is_done(Mmap2Stream_0__is_done),
  .Mmap2Stream_0__mmap_port_offset_in(a_offset),
  .Mmap2Stream_0__mmap_port_offset(Mmap2Stream_0__mmap_port_offset),
  .Mmap2Stream_1__ap_start(Mmap2Stream_1__ap_start),
  .Mmap2Stream_1__ap_ready(Mmap2Stream_1__ap_ready),
  .Mmap2Stream_1__ap_done(Mmap2Stream_1__ap_done),
  .Mmap2Stream_1__ap_idle(Mmap2Stream_1__ap_idle),
  .Mmap2Stream_1__is_done(Mmap2Stream_1__is_done),
  .Mmap2Stream_1__mmap_port_offset_in(b_offset),
  .Mmap2Stream_1__mmap_port_offset(Mmap2Stream_1__mmap_port_offset),
  .Stream2Mmap_0__ap_start(Stream2Mmap_0__ap_start),
  .Stream2Mmap_0__ap_ready(Stream2Mmap_0__ap_ready),
  .Stream2Mmap_0__ap_done(Stream2Mmap_0__ap_done),
  .Stream2Mmap_0__ap_idle(Stream2Mmap_0__ap_idle),
  .Stream2Mmap_0__is_done(Stream2Mmap_0__is_done),
  .Stream2Mmap_0__mmap_port_offset_in(c_offset),
  .Stream2Mmap_0__mmap_port_offset(Stream2Mmap_0__mmap_port_offset)
);

fifo #(
  .DATA_WIDTH(33),
  .ADDR_WIDTH(1),
  .DEPTH(2)
) a_q_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(a_q_dout),
  .if_empty_n(a_q_empty_n),
  .if_read(a_q_read),
  .if_read_ce(1'b1),
  .if_din(a_q_din),
  .if_full_n(a_q_full_n),
  .if_write(a_q_write),
  .if_write_ce(1'b1)
);

fifo #(
  .DATA_WIDTH(33),
  .ADDR_WIDTH(1),
  .DEPTH(2)
) b_q_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(b_q_dout),
  .if_empty_n(b_q_empty_n),
  .if_read(b_q_read),
  .if_read_ce(1'b1),
  .if_din(b_q_din),
  .if_full_n(b_q_full_n),
  .if_write(b_q_write),
  .if_write_ce(1'b1)
);

fifo #(
  .DATA_WIDTH(33),
  .ADDR_WIDTH(1),
  .DEPTH(2)
) c_q_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(c_q_dout),
  .if_empty_n(c_q_empty_n),
  .if_read(c_q_read),
  .if_read_ce(1'b1),
  .if_din(c_q_din),
  .if_full_n(c_q_full_n),
  .if_write(c_q_write),
  .if_write_ce(1'b1)
);

VecAdd_control_s_axi #(
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
  .a_offset(a_offset),
  .b_offset(b_offset),
  .c_offset(c_offset),
  .n(n)
);

assign ap_rst = !ap_rst_n;
endmodule //VecAdd
