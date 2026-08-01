module CustomShell (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire [63:0] n,
  output wire [32:0] out_s_din,
  input wire out_s_full_n,
  output wire out_s_write,
  input wire [32:0] out_peek
);


endmodule //CustomShell
