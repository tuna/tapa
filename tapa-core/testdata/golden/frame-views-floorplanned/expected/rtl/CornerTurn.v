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

(* max_fanout = 256 *) wire ap_rst;
wire Load_0__ap_start;
wire Load_0__ap_done;
wire Load_0__ap_idle;
wire Load_0__ap_ready;
wire Load_0__is_done;
wire [97:0] __tapa_control_Load_0__launch_input;
wire [97:0] __tapa_control_Load_0__launch_output;
wire __tapa_control_Load_0__reset_n;
wire __tapa_control_Load_0__local_completion;
wire [31:0] Load_0__n;
wire [63:0] Load_0__src_offset;
wire Store_0__ap_start;
wire Store_0__ap_done;
wire Store_0__ap_idle;
wire Store_0__ap_ready;
wire Store_0__is_done;
wire [97:0] __tapa_control_Store_0__launch_input;
wire [97:0] __tapa_control_Store_0__launch_output;
wire __tapa_control_Store_0__reset_n;
wire __tapa_control_Store_0__local_completion;
wire [63:0] Store_0__dst_offset;
wire [31:0] Store_0__n;
wire Turn_0__ap_start;
wire Turn_0__ap_done;
wire Turn_0__ap_idle;
wire Turn_0__ap_ready;
wire Turn_0__is_done;
wire [33:0] __tapa_control_Turn_0__launch_input;
wire [33:0] __tapa_control_Turn_0__launch_output;
wire __tapa_control_Turn_0__reset_n;
wire __tapa_control_Turn_0__local_completion;
wire [31:0] Turn_0__n;
wire TurnIn_0__ap_start;
wire TurnIn_0__ap_done;
wire TurnIn_0__ap_idle;
wire TurnIn_0__ap_ready;
wire TurnIn_0__is_done;
wire [33:0] __tapa_control_TurnIn_0__launch_input;
wire [33:0] __tapa_control_TurnIn_0__launch_output;
wire __tapa_control_TurnIn_0__reset_n;
wire __tapa_control_TurnIn_0__local_completion;
wire [31:0] TurnIn_0__n;
wire TurnOut_0__ap_start;
wire TurnOut_0__ap_done;
wire TurnOut_0__ap_idle;
wire TurnOut_0__ap_ready;
wire TurnOut_0__is_done;
wire [33:0] __tapa_control_TurnOut_0__launch_input;
wire [33:0] __tapa_control_TurnOut_0__launch_output;
wire __tapa_control_TurnOut_0__reset_n;
wire __tapa_control_TurnOut_0__local_completion;
wire [31:0] TurnOut_0__n;
wire [17:0] cols_Drain_0_CornerTurn_w_buf_address0;
wire cols_Drain_0_CornerTurn_w_buf_ce0;
wire cols_Drain_0_CornerTurn_w_buf_we0;
wire [31:0] cols_Drain_0_CornerTurn_w_buf_d0;
wire [7:0] cols_Drain_0_CornerTurn_w_acquire_dout;
wire cols_Drain_0_CornerTurn_w_acquire_empty_n;
wire cols_Drain_0_CornerTurn_w_acquire_read;
wire cols_Drain_0_CornerTurn_w_release_din;
wire cols_Drain_0_CornerTurn_w_release_full_n;
wire cols_Drain_0_CornerTurn_w_release_write;
wire [32:0] cols_Drain_0_CornerTurn_din;
wire cols_Drain_0_CornerTurn_full_n;
wire cols_Drain_0_CornerTurn_write;
wire [32:0] cols_Drain_0_CornerTurn_dout;
wire cols_Drain_0_CornerTurn_empty_n;
wire cols_Drain_0_CornerTurn_read;
wire [32:0] cols_Drain_0_CornerTurn_send_dout;
wire cols_Drain_0_CornerTurn_send_empty_n;
wire cols_Drain_0_CornerTurn_send_read;
wire [17:0] once_CornerTurn_w_buf_address0;
wire once_CornerTurn_w_buf_ce0;
wire once_CornerTurn_w_buf_we0;
wire [31:0] once_CornerTurn_w_buf_d0;
wire [7:0] once_CornerTurn_w_acquire_dout;
wire once_CornerTurn_w_acquire_empty_n;
wire once_CornerTurn_w_acquire_read;
wire once_CornerTurn_w_release_din;
wire once_CornerTurn_w_release_full_n;
wire once_CornerTurn_w_release_write;
wire [17:0] once_CornerTurn_r_buf_address0;
wire once_CornerTurn_r_buf_ce0;
wire [31:0] once_CornerTurn_r_buf_q0;
wire [7:0] once_CornerTurn_r_acquire_dout;
wire once_CornerTurn_r_acquire_empty_n;
wire once_CornerTurn_r_acquire_read;
wire once_CornerTurn_r_release_din;
wire once_CornerTurn_r_release_full_n;
wire once_CornerTurn_r_release_write;
wire [17:0] rows_CornerTurn_r_buf_address0;
wire rows_CornerTurn_r_buf_ce0;
wire [31:0] rows_CornerTurn_r_buf_q0;
wire [7:0] rows_CornerTurn_r_acquire_dout;
wire rows_CornerTurn_r_acquire_empty_n;
wire rows_CornerTurn_r_acquire_read;
wire rows_CornerTurn_r_release_din;
wire rows_CornerTurn_r_release_full_n;
wire rows_CornerTurn_r_release_write;
wire [32:0] rows_CornerTurn_din;
wire rows_CornerTurn_full_n;
wire rows_CornerTurn_write;
wire [32:0] rows_CornerTurn_dout;
wire rows_CornerTurn_empty_n;
wire rows_CornerTurn_read;
wire [32:0] rows_CornerTurn_recv_din;
wire rows_CornerTurn_recv_full_n;
wire rows_CornerTurn_recv_write;
wire [32:0] twice_CornerTurn_dout;
wire twice_CornerTurn_empty_n;
wire twice_CornerTurn_read;
wire [32:0] twice_CornerTurn_din;
wire twice_CornerTurn_full_n;
wire twice_CornerTurn_write;
wire __tapa_control_start;
wire __tapa_control_release;
wire __tapa_control_children_done;
wire __tapa_control_children_clear;
wire __tapa_control_fabric_reset_n;
wire [63:0] src_offset;
wire [63:0] dst_offset;
wire [31:0] n;

tapa_control_pipeline #(
  .WIDTH(98),
  .BODY_LEVEL(4)
) __tapa_control_Load_0_launch (
  .clk(ap_clk),
  .in_data(__tapa_control_Load_0__launch_input),
  .out_data(__tapa_control_Load_0__launch_output)
);

tapa_control_pipeline #(
  .WIDTH(1),
  .BODY_LEVEL(4)
) __tapa_control_Load_0_reset (
  .clk(ap_clk),
  .in_data(ap_rst_n),
  .out_data(__tapa_control_Load_0__reset_n)
);

tapa_local_controller #(
  .AUTORUN(0)
) __tapa_local_controller_Load_0 (
  .ap_clk(ap_clk),
  .reset_n(__tapa_control_Load_0__reset_n),
  .launch_start(__tapa_control_Load_0__launch_output[0]),
  .launch_release(__tapa_control_Load_0__launch_output[1]),
  .child_done(Load_0__ap_done),
  .child_ready(Load_0__ap_ready),
  .child_idle(Load_0__ap_idle),
  .child_start(Load_0__ap_start),
  .completion(__tapa_control_Load_0__local_completion)
);

tapa_control_pipeline #(
  .WIDTH(1),
  .BODY_LEVEL(4)
) __tapa_control_Load_0_completion (
  .clk(ap_clk),
  .in_data(__tapa_control_Load_0__local_completion),
  .out_data(Load_0__is_done)
);

Load Load_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(__tapa_control_Load_0__reset_n),
  .ap_start(Load_0__ap_start),
  .ap_done(Load_0__ap_done),
  .ap_idle(Load_0__ap_idle),
  .ap_ready(Load_0__ap_ready),
  .n(Load_0__n),
  .out_s_din(rows_CornerTurn_din),
  .out_s_full_n(rows_CornerTurn_full_n),
  .out_s_write(rows_CornerTurn_write),
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

tapa_control_pipeline #(
  .WIDTH(98),
  .BODY_LEVEL(6)
) __tapa_control_Store_0_launch (
  .clk(ap_clk),
  .in_data(__tapa_control_Store_0__launch_input),
  .out_data(__tapa_control_Store_0__launch_output)
);

tapa_control_pipeline #(
  .WIDTH(1),
  .BODY_LEVEL(6)
) __tapa_control_Store_0_reset (
  .clk(ap_clk),
  .in_data(ap_rst_n),
  .out_data(__tapa_control_Store_0__reset_n)
);

tapa_local_controller #(
  .AUTORUN(0)
) __tapa_local_controller_Store_0 (
  .ap_clk(ap_clk),
  .reset_n(__tapa_control_Store_0__reset_n),
  .launch_start(__tapa_control_Store_0__launch_output[0]),
  .launch_release(__tapa_control_Store_0__launch_output[1]),
  .child_done(Store_0__ap_done),
  .child_ready(Store_0__ap_ready),
  .child_idle(Store_0__ap_idle),
  .child_start(Store_0__ap_start),
  .completion(__tapa_control_Store_0__local_completion)
);

tapa_control_pipeline #(
  .WIDTH(1),
  .BODY_LEVEL(2)
) __tapa_control_Store_0_completion (
  .clk(ap_clk),
  .in_data(__tapa_control_Store_0__local_completion),
  .out_data(Store_0__is_done)
);

Store Store_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(__tapa_control_Store_0__reset_n),
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
  .in_s_dout(cols_Drain_0_CornerTurn_dout),
  .in_peek_dout(cols_Drain_0_CornerTurn_dout),
  .in_s_empty_n(cols_Drain_0_CornerTurn_empty_n),
  .in_peek_empty_n(cols_Drain_0_CornerTurn_empty_n),
  .in_s_read(cols_Drain_0_CornerTurn_read),
  .n(Store_0__n)
);

tapa_local_controller #(
  .AUTORUN(0)
) __tapa_local_controller_Turn_0 (
  .ap_clk(ap_clk),
  .reset_n(__tapa_control_Turn_0__reset_n),
  .launch_start(__tapa_control_Turn_0__launch_output[0]),
  .launch_release(__tapa_control_Turn_0__launch_output[1]),
  .child_done(Turn_0__ap_done),
  .child_ready(Turn_0__ap_ready),
  .child_idle(Turn_0__ap_idle),
  .child_start(Turn_0__ap_start),
  .completion(__tapa_control_Turn_0__local_completion)
);

Turn Turn_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(__tapa_control_Turn_0__reset_n),
  .ap_start(Turn_0__ap_start),
  .ap_done(Turn_0__ap_done),
  .ap_idle(Turn_0__ap_idle),
  .ap_ready(Turn_0__ap_ready),
  .in_buf_address0(rows_CornerTurn_r_buf_address0),
  .in_buf_ce0(rows_CornerTurn_r_buf_ce0),
  .in_buf_q0(rows_CornerTurn_r_buf_q0),
  .in_acquire_dout(rows_CornerTurn_r_acquire_dout),
  .in_acquire_empty_n(rows_CornerTurn_r_acquire_empty_n),
  .in_acquire_read(rows_CornerTurn_r_acquire_read),
  .in_release_din(rows_CornerTurn_r_release_din),
  .in_release_full_n(rows_CornerTurn_r_release_full_n),
  .in_release_write(rows_CornerTurn_r_release_write),
  .n(Turn_0__n),
  .out_buf_address0(once_CornerTurn_w_buf_address0),
  .out_buf_ce0(once_CornerTurn_w_buf_ce0),
  .out_buf_we0(once_CornerTurn_w_buf_we0),
  .out_buf_d0(once_CornerTurn_w_buf_d0),
  .out_acquire_dout(once_CornerTurn_w_acquire_dout),
  .out_acquire_empty_n(once_CornerTurn_w_acquire_empty_n),
  .out_acquire_read(once_CornerTurn_w_acquire_read),
  .out_release_din(once_CornerTurn_w_release_din),
  .out_release_full_n(once_CornerTurn_w_release_full_n),
  .out_release_write(once_CornerTurn_w_release_write)
);

tapa_local_controller #(
  .AUTORUN(0)
) __tapa_local_controller_TurnIn_0 (
  .ap_clk(ap_clk),
  .reset_n(__tapa_control_TurnIn_0__reset_n),
  .launch_start(__tapa_control_TurnIn_0__launch_output[0]),
  .launch_release(__tapa_control_TurnIn_0__launch_output[1]),
  .child_done(TurnIn_0__ap_done),
  .child_ready(TurnIn_0__ap_ready),
  .child_idle(TurnIn_0__ap_idle),
  .child_start(TurnIn_0__ap_start),
  .completion(__tapa_control_TurnIn_0__local_completion)
);

TurnIn TurnIn_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(__tapa_control_TurnIn_0__reset_n),
  .ap_start(TurnIn_0__ap_start),
  .ap_done(TurnIn_0__ap_done),
  .ap_idle(TurnIn_0__ap_idle),
  .ap_ready(TurnIn_0__ap_ready),
  .in_s_dout(twice_CornerTurn_dout),
  .in_peek_dout(twice_CornerTurn_dout),
  .in_s_empty_n(twice_CornerTurn_empty_n),
  .in_peek_empty_n(twice_CornerTurn_empty_n),
  .in_s_read(twice_CornerTurn_read),
  .n(TurnIn_0__n),
  .out_buf_address0(cols_Drain_0_CornerTurn_w_buf_address0),
  .out_buf_ce0(cols_Drain_0_CornerTurn_w_buf_ce0),
  .out_buf_we0(cols_Drain_0_CornerTurn_w_buf_we0),
  .out_buf_d0(cols_Drain_0_CornerTurn_w_buf_d0),
  .out_acquire_dout(cols_Drain_0_CornerTurn_w_acquire_dout),
  .out_acquire_empty_n(cols_Drain_0_CornerTurn_w_acquire_empty_n),
  .out_acquire_read(cols_Drain_0_CornerTurn_w_acquire_read),
  .out_release_din(cols_Drain_0_CornerTurn_w_release_din),
  .out_release_full_n(cols_Drain_0_CornerTurn_w_release_full_n),
  .out_release_write(cols_Drain_0_CornerTurn_w_release_write)
);

tapa_local_controller #(
  .AUTORUN(0)
) __tapa_local_controller_TurnOut_0 (
  .ap_clk(ap_clk),
  .reset_n(__tapa_control_TurnOut_0__reset_n),
  .launch_start(__tapa_control_TurnOut_0__launch_output[0]),
  .launch_release(__tapa_control_TurnOut_0__launch_output[1]),
  .child_done(TurnOut_0__ap_done),
  .child_ready(TurnOut_0__ap_ready),
  .child_idle(TurnOut_0__ap_idle),
  .child_start(TurnOut_0__ap_start),
  .completion(__tapa_control_TurnOut_0__local_completion)
);

TurnOut TurnOut_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(__tapa_control_TurnOut_0__reset_n),
  .ap_start(TurnOut_0__ap_start),
  .ap_done(TurnOut_0__ap_done),
  .ap_idle(TurnOut_0__ap_idle),
  .ap_ready(TurnOut_0__ap_ready),
  .in_buf_address0(once_CornerTurn_r_buf_address0),
  .in_buf_ce0(once_CornerTurn_r_buf_ce0),
  .in_buf_q0(once_CornerTurn_r_buf_q0),
  .in_acquire_dout(once_CornerTurn_r_acquire_dout),
  .in_acquire_empty_n(once_CornerTurn_r_acquire_empty_n),
  .in_acquire_read(once_CornerTurn_r_acquire_read),
  .in_release_din(once_CornerTurn_r_release_din),
  .in_release_full_n(once_CornerTurn_r_release_full_n),
  .in_release_write(once_CornerTurn_r_release_write),
  .n(TurnOut_0__n),
  .out_s_din(twice_CornerTurn_din),
  .out_s_full_n(twice_CornerTurn_full_n),
  .out_s_write(twice_CornerTurn_write),
  .out_peek('d0)
);

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(1),
  .R_VIEW(0)
) cols_Drain_0_CornerTurn_send (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0(cols_Drain_0_CornerTurn_w_buf_address0),
  .w_buf_ce0(cols_Drain_0_CornerTurn_w_buf_ce0),
  .w_buf_we0(cols_Drain_0_CornerTurn_w_buf_we0),
  .w_buf_d0(cols_Drain_0_CornerTurn_w_buf_d0),
  .w_acquire_dout(cols_Drain_0_CornerTurn_w_acquire_dout),
  .w_acquire_empty_n(cols_Drain_0_CornerTurn_w_acquire_empty_n),
  .w_acquire_read(cols_Drain_0_CornerTurn_w_acquire_read),
  .w_release_full_n(cols_Drain_0_CornerTurn_w_release_full_n),
  .w_release_write(cols_Drain_0_CornerTurn_w_release_write),
  .if_din('d0),
  .if_write('d0),
  .r_buf_address0('d0),
  .r_buf_ce0('d0),
  .r_acquire_read('d0),
  .r_release_write('d0),
  .if_dout(cols_Drain_0_CornerTurn_send_dout),
  .if_empty_n(cols_Drain_0_CornerTurn_send_empty_n),
  .if_read(cols_Drain_0_CornerTurn_send_read)
);

tapa_hs_pipeline #(
  .DATA_WIDTH(33),
  .DEPTH(2),
  .BODY_LEVEL(2)
) cols_Drain_0_CornerTurn_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(cols_Drain_0_CornerTurn_dout),
  .if_empty_n(cols_Drain_0_CornerTurn_empty_n),
  .if_read(cols_Drain_0_CornerTurn_read),
  .if_read_ce(1'b1),
  .if_din(cols_Drain_0_CornerTurn_din),
  .if_full_n(cols_Drain_0_CornerTurn_full_n),
  .if_write(cols_Drain_0_CornerTurn_write),
  .if_write_ce(1'b1)
);

frame_channel #(
  .DATA_WIDTH(32),
  .FRAME(1024),
  .BANKS(2),
  .ADDR_WIDTH(18),
  .BANK_WIDTH(8),
  .W_VIEW(1),
  .R_VIEW(1)
) once_CornerTurn_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0(once_CornerTurn_w_buf_address0),
  .w_buf_ce0(once_CornerTurn_w_buf_ce0),
  .w_buf_we0(once_CornerTurn_w_buf_we0),
  .w_buf_d0(once_CornerTurn_w_buf_d0),
  .w_acquire_dout(once_CornerTurn_w_acquire_dout),
  .w_acquire_empty_n(once_CornerTurn_w_acquire_empty_n),
  .w_acquire_read(once_CornerTurn_w_acquire_read),
  .w_release_full_n(once_CornerTurn_w_release_full_n),
  .w_release_write(once_CornerTurn_w_release_write),
  .if_din('d0),
  .if_write('d0),
  .r_buf_address0(once_CornerTurn_r_buf_address0),
  .r_buf_ce0(once_CornerTurn_r_buf_ce0),
  .r_buf_q0(once_CornerTurn_r_buf_q0),
  .r_acquire_dout(once_CornerTurn_r_acquire_dout),
  .r_acquire_empty_n(once_CornerTurn_r_acquire_empty_n),
  .r_acquire_read(once_CornerTurn_r_acquire_read),
  .r_release_full_n(once_CornerTurn_r_release_full_n),
  .r_release_write(once_CornerTurn_r_release_write),
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
) rows_CornerTurn_recv (
  .clk(ap_clk),
  .reset(ap_rst),
  .w_buf_address0('d0),
  .w_buf_ce0('d0),
  .w_buf_we0('d0),
  .w_buf_d0('d0),
  .w_acquire_read('d0),
  .w_release_write('d0),
  .if_din(rows_CornerTurn_recv_din),
  .if_full_n(rows_CornerTurn_recv_full_n),
  .if_write(rows_CornerTurn_recv_write),
  .r_buf_address0(rows_CornerTurn_r_buf_address0),
  .r_buf_ce0(rows_CornerTurn_r_buf_ce0),
  .r_buf_q0(rows_CornerTurn_r_buf_q0),
  .r_acquire_dout(rows_CornerTurn_r_acquire_dout),
  .r_acquire_empty_n(rows_CornerTurn_r_acquire_empty_n),
  .r_acquire_read(rows_CornerTurn_r_acquire_read),
  .r_release_full_n(rows_CornerTurn_r_release_full_n),
  .r_release_write(rows_CornerTurn_r_release_write),
  .if_read('d0)
);

tapa_hs_pipeline #(
  .DATA_WIDTH(33),
  .DEPTH(2),
  .BODY_LEVEL(4)
) rows_CornerTurn_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(rows_CornerTurn_dout),
  .if_empty_n(rows_CornerTurn_empty_n),
  .if_read(rows_CornerTurn_read),
  .if_read_ce(1'b1),
  .if_din(rows_CornerTurn_din),
  .if_full_n(rows_CornerTurn_full_n),
  .if_write(rows_CornerTurn_write),
  .if_write_ce(1'b1)
);

fifo_almost_full #(
  .DATA_WIDTH(33),
  .ADDR_WIDTH(3),
  .DEPTH(7),
  .GRACE_PERIOD(1)
) twice_CornerTurn_fifo (
  .clk(ap_clk),
  .reset(ap_rst),
  .if_dout(twice_CornerTurn_dout),
  .if_empty_n(twice_CornerTurn_empty_n),
  .if_read(twice_CornerTurn_read),
  .if_read_ce(1'b1),
  .if_din(twice_CornerTurn_din),
  .if_full_n(twice_CornerTurn_full_n),
  .if_write(twice_CornerTurn_write),
  .if_write_ce(1'b1)
);

tapa_global_controller #(
  .FLUSH_CYCLES(14)
) __tapa_global_controller (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .children_done(__tapa_control_children_done),
  .children_clear(__tapa_control_children_clear),
  .launch_start(__tapa_control_start),
  .launch_release(__tapa_control_release),
  .fabric_reset_n(__tapa_control_fabric_reset_n),
  .ap_done(ap_done),
  .ap_ready(ap_ready),
  .ap_idle(ap_idle)
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

assign ap_rst = !__tapa_control_fabric_reset_n;
assign __tapa_control_Load_0__launch_input = {src_offset, n, __tapa_control_release, __tapa_control_start};
assign Load_0__n = __tapa_control_Load_0__launch_output[33:2];
assign Load_0__src_offset = __tapa_control_Load_0__launch_output[97:34];
assign __tapa_control_Store_0__launch_input = {n, dst_offset, __tapa_control_release, __tapa_control_start};
assign Store_0__dst_offset = __tapa_control_Store_0__launch_output[65:2];
assign Store_0__n = __tapa_control_Store_0__launch_output[97:66];
assign __tapa_control_Turn_0__launch_input = {n, __tapa_control_release, __tapa_control_start};
assign __tapa_control_Turn_0__launch_output = __tapa_control_Turn_0__launch_input;
assign __tapa_control_Turn_0__reset_n = ap_rst_n;
assign Turn_0__n = __tapa_control_Turn_0__launch_output[33:2];
assign Turn_0__is_done = __tapa_control_Turn_0__local_completion;
assign __tapa_control_TurnIn_0__launch_input = {n, __tapa_control_release, __tapa_control_start};
assign __tapa_control_TurnIn_0__launch_output = __tapa_control_TurnIn_0__launch_input;
assign __tapa_control_TurnIn_0__reset_n = ap_rst_n;
assign TurnIn_0__n = __tapa_control_TurnIn_0__launch_output[33:2];
assign TurnIn_0__is_done = __tapa_control_TurnIn_0__local_completion;
assign __tapa_control_TurnOut_0__launch_input = {n, __tapa_control_release, __tapa_control_start};
assign __tapa_control_TurnOut_0__launch_output = __tapa_control_TurnOut_0__launch_input;
assign __tapa_control_TurnOut_0__reset_n = ap_rst_n;
assign TurnOut_0__n = __tapa_control_TurnOut_0__launch_output[33:2];
assign TurnOut_0__is_done = __tapa_control_TurnOut_0__local_completion;
assign cols_Drain_0_CornerTurn_din = cols_Drain_0_CornerTurn_send_dout;
assign cols_Drain_0_CornerTurn_write = (cols_Drain_0_CornerTurn_send_empty_n && cols_Drain_0_CornerTurn_full_n);
assign cols_Drain_0_CornerTurn_send_read = (cols_Drain_0_CornerTurn_send_empty_n && cols_Drain_0_CornerTurn_full_n);
assign rows_CornerTurn_recv_din = rows_CornerTurn_dout;
assign rows_CornerTurn_recv_write = (rows_CornerTurn_empty_n && rows_CornerTurn_recv_full_n);
assign rows_CornerTurn_read = (rows_CornerTurn_empty_n && rows_CornerTurn_recv_full_n);
assign m_axi_dst_AWLOCK = 1'b0;
assign m_axi_dst_AWCACHE = 4'b0011;
assign m_axi_dst_AWPROT = 3'b000;
assign m_axi_dst_AWQOS = 4'b0000;
assign m_axi_dst_ARLOCK = 1'b0;
assign m_axi_dst_ARCACHE = 4'b0011;
assign m_axi_dst_ARPROT = 3'b000;
assign m_axi_dst_ARQOS = 4'b0000;
assign m_axi_src_AWLOCK = 1'b0;
assign m_axi_src_AWCACHE = 4'b0011;
assign m_axi_src_AWPROT = 3'b000;
assign m_axi_src_AWQOS = 4'b0000;
assign m_axi_src_ARLOCK = 1'b0;
assign m_axi_src_ARCACHE = 4'b0011;
assign m_axi_src_ARPROT = 3'b000;
assign m_axi_src_ARQOS = 4'b0000;
assign __tapa_control_children_done = ((((Load_0__is_done && Store_0__is_done) && Turn_0__is_done) && TurnIn_0__is_done) && TurnOut_0__is_done);
assign __tapa_control_children_clear = ((((!Load_0__is_done && !Store_0__is_done) && !Turn_0__is_done) && !TurnIn_0__is_done) && !TurnOut_0__is_done);
endmodule //CornerTurn
