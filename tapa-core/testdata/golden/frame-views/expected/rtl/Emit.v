module Emit (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  output wire [32:0] twice_s_din,
  input wire twice_s_full_n,
  output wire twice_s_write,
  input wire [32:0] twice_peek,
  input wire [31:0] n,
  output wire [17:0] once_buf_address0,
  output wire once_buf_ce0,
  input wire [31:0] once_buf_q0,
  input wire [7:0] once_acquire_dout,
  input wire once_acquire_empty_n,
  output wire once_acquire_read,
  output wire once_release_din,
  input wire once_release_full_n,
  output wire once_release_write
);

wire ap_rst;
reg [1:0] TurnOut_0__state;
wire TurnOut_0__ap_start;
wire TurnOut_0__ap_done;
wire TurnOut_0__is_done;
wire TurnOut_0__ap_idle;
wire TurnOut_0__ap_ready;
wire [31:0] TurnOut_0__n;

TurnOut TurnOut_0 (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(TurnOut_0__ap_start),
  .ap_done(TurnOut_0__ap_done),
  .ap_idle(TurnOut_0__ap_idle),
  .ap_ready(TurnOut_0__ap_ready),
  .in_buf_address0(once_buf_address0),
  .in_buf_ce0(once_buf_ce0),
  .in_buf_q0(once_buf_q0),
  .in_acquire_dout(once_acquire_dout),
  .in_acquire_empty_n(once_acquire_empty_n),
  .in_acquire_read(once_acquire_read),
  .in_release_din(once_release_din),
  .in_release_full_n(once_release_full_n),
  .in_release_write(once_release_write),
  .n(TurnOut_0__n),
  .out_s_din(twice_s_din),
  .out_s_full_n(twice_s_full_n),
  .out_s_write(twice_s_write),
  .out_peek('d0)
);

Emit_fsm __tapa_fsm_unit (
  .ap_clk(ap_clk),
  .ap_rst_n(ap_rst_n),
  .ap_start(ap_start),
  .ap_done(ap_done),
  .ap_idle(ap_idle),
  .ap_ready(ap_ready),
  .TurnOut_0__ap_start(TurnOut_0__ap_start),
  .TurnOut_0__ap_ready(TurnOut_0__ap_ready),
  .TurnOut_0__ap_done(TurnOut_0__ap_done),
  .TurnOut_0__ap_idle(TurnOut_0__ap_idle),
  .TurnOut_0__is_done(TurnOut_0__is_done),
  .TurnOut_0__n_in(n),
  .TurnOut_0__n(TurnOut_0__n)
);

assign ap_rst = !ap_rst_n;
endmodule //Emit
