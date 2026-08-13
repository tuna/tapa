module ShellTop #(
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
  output wire [32:0] out_din,
  input wire out_full_n,
  output wire out_write
);

wire ap_rst;
reg [1:0] CustomShell_0__state;
wire CustomShell_0__ap_start;
wire CustomShell_0__ap_done;
wire CustomShell_0__is_done;
wire CustomShell_0__ap_idle;
wire CustomShell_0__ap_ready;
wire [63:0] CustomShell_0__n;
wire [63:0] n;

CustomShell CustomShell_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(CustomShell_0__ap_start),
  .ap_done(CustomShell_0__ap_done),
  .ap_idle(CustomShell_0__ap_idle),
  .ap_ready(CustomShell_0__ap_ready),
  .n(CustomShell_0__n),
  .out_s_din(out_din),
  .out_s_full_n(out_full_n),
  .out_s_write(out_write),
  .out_peek('d0)
);

ShellTop_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .CustomShell_0__ap_start(CustomShell_0__ap_start),
  .CustomShell_0__ap_ready(CustomShell_0__ap_ready),
  .CustomShell_0__ap_done(CustomShell_0__ap_done),
  .CustomShell_0__ap_idle(CustomShell_0__ap_idle),
  .CustomShell_0__is_done(CustomShell_0__is_done),
  .CustomShell_0__n_in(n),
  .CustomShell_0__n(CustomShell_0__n)
);

ShellTop_control_s_axi #(
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
  .n(n)
);

assign ap_rst = !ap_rst_n;
endmodule //ShellTop
