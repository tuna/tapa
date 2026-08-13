module AsyncTop #(
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
  output wire [32:0] data_q_din,
  input wire data_q_full_n,
  output wire data_q_write,
  output wire [63:0] m_axi_mem_ARADDR,
  output wire [1:0] m_axi_mem_ARBURST,
  output wire [3:0] m_axi_mem_ARCACHE,
  output wire m_axi_mem_ARID,
  output wire [7:0] m_axi_mem_ARLEN,
  output wire m_axi_mem_ARLOCK,
  output wire [2:0] m_axi_mem_ARPROT,
  output wire [3:0] m_axi_mem_ARQOS,
  input wire m_axi_mem_ARREADY,
  output wire [2:0] m_axi_mem_ARSIZE,
  output wire m_axi_mem_ARVALID,
  output wire [63:0] m_axi_mem_AWADDR,
  output wire [1:0] m_axi_mem_AWBURST,
  output wire [3:0] m_axi_mem_AWCACHE,
  output wire m_axi_mem_AWID,
  output wire [7:0] m_axi_mem_AWLEN,
  output wire m_axi_mem_AWLOCK,
  output wire [2:0] m_axi_mem_AWPROT,
  output wire [3:0] m_axi_mem_AWQOS,
  input wire m_axi_mem_AWREADY,
  output wire [2:0] m_axi_mem_AWSIZE,
  output wire m_axi_mem_AWVALID,
  input wire m_axi_mem_BID,
  output wire m_axi_mem_BREADY,
  input wire [1:0] m_axi_mem_BRESP,
  input wire m_axi_mem_BVALID,
  input wire [31:0] m_axi_mem_RDATA,
  input wire m_axi_mem_RID,
  input wire m_axi_mem_RLAST,
  output wire m_axi_mem_RREADY,
  input wire [1:0] m_axi_mem_RRESP,
  input wire m_axi_mem_RVALID,
  output wire [31:0] m_axi_mem_WDATA,
  output wire m_axi_mem_WLAST,
  input wire m_axi_mem_WREADY,
  output wire [3:0] m_axi_mem_WSTRB,
  output wire m_axi_mem_WVALID
);

wire ap_rst;
reg [1:0] AsyncReader_0__state;
wire AsyncReader_0__ap_start;
wire AsyncReader_0__ap_done;
wire AsyncReader_0__is_done;
wire AsyncReader_0__ap_idle;
wire AsyncReader_0__ap_ready;
wire [63:0] AsyncReader_0__mem_offset;
wire [63:0] AsyncReader_0__n;
wire [63:0] mem_read_addr__din;
wire mem_read_addr__full_n;
wire mem_read_addr__write;
wire [31:0] mem_read_data__dout;
wire mem_read_data__empty_n;
wire mem_read_data__read;
wire [63:0] mem_write_addr__din;
wire mem_write_addr__full_n;
wire mem_write_addr__write;
wire [32:0] mem_write_data__din;
wire mem_write_data__full_n;
wire mem_write_data__write;
wire [7:0] mem_write_resp__dout;
wire mem_write_resp__empty_n;
wire mem_write_resp__read;
wire [63:0] mem_offset;
wire [63:0] n;

async_mmap #(
  .DataWidth(32),
  .DataWidthBytesLog(2),
  .AddrWidth(64),
  .WaitTimeWidth(2),
  .MaxWaitTime(3),
  .BurstLenWidth(8),
  .MaxBurstLen(255),
  .EnableReadChannel(1),
  .EnableWriteChannel(0)
) mem__m_axi (
  .clk(ap_clk),
  .rst(ap_rst),
  .m_axi_AWADDR(m_axi_mem_AWADDR),
  .m_axi_AWBURST(m_axi_mem_AWBURST),
  .m_axi_AWCACHE(m_axi_mem_AWCACHE),
  .m_axi_AWID(m_axi_mem_AWID),
  .m_axi_AWLEN(m_axi_mem_AWLEN),
  .m_axi_AWLOCK(m_axi_mem_AWLOCK),
  .m_axi_AWPROT(m_axi_mem_AWPROT),
  .m_axi_AWQOS(m_axi_mem_AWQOS),
  .m_axi_AWREADY(m_axi_mem_AWREADY),
  .m_axi_AWSIZE(m_axi_mem_AWSIZE),
  .m_axi_AWVALID(m_axi_mem_AWVALID),
  .m_axi_WDATA(m_axi_mem_WDATA),
  .m_axi_WLAST(m_axi_mem_WLAST),
  .m_axi_WREADY(m_axi_mem_WREADY),
  .m_axi_WSTRB(m_axi_mem_WSTRB),
  .m_axi_WVALID(m_axi_mem_WVALID),
  .m_axi_BID(m_axi_mem_BID),
  .m_axi_BREADY(m_axi_mem_BREADY),
  .m_axi_BRESP(m_axi_mem_BRESP),
  .m_axi_BVALID(m_axi_mem_BVALID),
  .m_axi_ARADDR(m_axi_mem_ARADDR),
  .m_axi_ARBURST(m_axi_mem_ARBURST),
  .m_axi_ARCACHE(m_axi_mem_ARCACHE),
  .m_axi_ARID(m_axi_mem_ARID),
  .m_axi_ARLEN(m_axi_mem_ARLEN),
  .m_axi_ARLOCK(m_axi_mem_ARLOCK),
  .m_axi_ARPROT(m_axi_mem_ARPROT),
  .m_axi_ARQOS(m_axi_mem_ARQOS),
  .m_axi_ARREADY(m_axi_mem_ARREADY),
  .m_axi_ARSIZE(m_axi_mem_ARSIZE),
  .m_axi_ARVALID(m_axi_mem_ARVALID),
  .m_axi_RDATA(m_axi_mem_RDATA),
  .m_axi_RID(m_axi_mem_RID),
  .m_axi_RLAST(m_axi_mem_RLAST),
  .m_axi_RREADY(m_axi_mem_RREADY),
  .m_axi_RRESP(m_axi_mem_RRESP),
  .m_axi_RVALID(m_axi_mem_RVALID),
  .read_addr_din(mem_read_addr__din),
  .read_addr_full_n(mem_read_addr__full_n),
  .read_addr_write(mem_read_addr__write),
  .read_data_dout(mem_read_data__dout),
  .read_data_empty_n(mem_read_data__empty_n),
  .read_data_read(mem_read_data__read),
  .write_addr_din(mem_write_addr__din),
  .write_addr_full_n(mem_write_addr__full_n),
  .write_addr_write(mem_write_addr__write),
  .write_data_din(mem_write_data__din[31:0]),
  .write_data_full_n(mem_write_data__full_n),
  .write_data_write(mem_write_data__write),
  .write_resp_dout(mem_write_resp__dout),
  .write_resp_empty_n(mem_write_resp__empty_n),
  .write_resp_read(mem_write_resp__read)
);

AsyncReader AsyncReader_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(AsyncReader_0__ap_start),
  .ap_done(AsyncReader_0__ap_done),
  .ap_idle(AsyncReader_0__ap_idle),
  .ap_ready(AsyncReader_0__ap_ready),
  .data_q_din(data_q_din),
  .data_q_full_n(data_q_full_n),
  .data_q_write(data_q_write),
  .mem_read_addr_s_din(mem_read_addr__din),
  .mem_read_addr_s_full_n(mem_read_addr__full_n),
  .mem_read_addr_s_write(mem_read_addr__write),
  .mem_read_addr_offset(AsyncReader_0__mem_offset),
  .mem_read_data_s_dout({1'b0, mem_read_data__dout}),
  .mem_read_data_peek_dout({1'b0, mem_read_data__dout}),
  .mem_read_data_s_empty_n(mem_read_data__empty_n),
  .mem_read_data_peek_empty_n(mem_read_data__empty_n),
  .mem_read_data_s_read(mem_read_data__read),
  .mem_write_addr_s_write(mem_write_addr__write),
  .mem_write_data_s_write(mem_write_data__write),
  .mem_write_resp_s_read(mem_write_resp__read),
  .n(AsyncReader_0__n)
);

AsyncTop_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .AsyncReader_0__ap_start(AsyncReader_0__ap_start),
  .AsyncReader_0__ap_ready(AsyncReader_0__ap_ready),
  .AsyncReader_0__ap_done(AsyncReader_0__ap_done),
  .AsyncReader_0__ap_idle(AsyncReader_0__ap_idle),
  .AsyncReader_0__is_done(AsyncReader_0__is_done),
  .AsyncReader_0__mem_offset_in(mem_offset),
  .AsyncReader_0__mem_offset(AsyncReader_0__mem_offset),
  .AsyncReader_0__n_in(n),
  .AsyncReader_0__n(AsyncReader_0__n)
);

AsyncTop_control_s_axi #(
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
  .mem_offset(mem_offset),
  .n(n)
);

assign ap_rst = !ap_rst_n;
endmodule //AsyncTop
