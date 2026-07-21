`default_nettype none

// Floorplanning-friendly first-word-fall-through stream storage.
//
// This uses a Head/Body/Tail organization expressed with a generate loop, so
// BODY_LEVEL is not limited by a statically unrolled template. Head and every
// Body pipeline ready/valid/data, and Tail is the enlarged almost-full FIFO
// that absorbs the resulting in-flight transactions.
(* keep_hierarchy = "yes" *)
module tapa_hs_pipeline #(
  parameter DATA_WIDTH = 32,
  parameter DEPTH = 24,
  parameter BODY_LEVEL = 2,
  parameter GRACE_PERIOD = BODY_LEVEL * 2 + 2,
  parameter REAL_DEPTH = GRACE_PERIOD + DEPTH + 4,
  parameter REAL_ADDR_WIDTH = $clog2(REAL_DEPTH)
) (
  input wire clk,
  input wire reset,

  output wire                  if_full_n,
  input  wire                  if_write_ce,
  input  wire                  if_write,
  input  wire [DATA_WIDTH-1:0] if_din,

  output wire                  if_empty_n,
  input  wire                  if_read_ce,
  input  wire                  if_read,
  output wire [DATA_WIDTH-1:0] if_dout
);

  wire                  head_gate_valid;
  wire                  head_gate_ready;
  wire [DATA_WIDTH-1:0] head_gate_data;
  wire                  active_write;
  wire                  active_read;

  wire                  body_valid [0:BODY_LEVEL];
  wire                  body_ready [0:BODY_LEVEL];
  wire [DATA_WIDTH-1:0] body_data [0:BODY_LEVEL];

  assign active_write = if_write & if_write_ce;
  assign active_read = if_read & if_read_ce;

  // The gate prevents a transaction from entering while the pipelined ready
  // value is low.  It is combinational and intentionally shares Head's slot.
  (* keep_hierarchy = "yes" *)
  tapa_hs_pipeline_head_gate #(
    .DATA_WIDTH(DATA_WIDTH)
  ) TAPA_HS_HEAD_GATE (
    .if_full_n(if_full_n),
    .if_write(active_write),
    .if_din(if_din),
    .if_empty_n(head_gate_valid),
    .if_read(head_gate_ready),
    .if_dout(head_gate_data)
  );

  (* keep_hierarchy = "yes" *)
  tapa_hs_pipeline_head #(
    .DATA_WIDTH(DATA_WIDTH)
  ) TAPA_HS_HEAD (
    .clk(clk),
    .if_full_n(head_gate_ready),
    .if_write(head_gate_valid),
    .if_din(head_gate_data),
    .if_empty_n(body_valid[0]),
    .if_read(body_ready[0]),
    .if_dout(body_data[0])
  );

  genvar i;
  generate
    for (i = 0; i < BODY_LEVEL; i = i + 1) begin : TAPA_HS_BODY
      (* keep_hierarchy = "yes" *)
      tapa_hs_pipeline_body #(
        .DATA_WIDTH(DATA_WIDTH)
      ) TAPA_HS_BODY_REG (
        .clk(clk),
        .if_full_n(body_ready[i]),
        .if_write(body_valid[i]),
        .if_din(body_data[i]),
        .if_empty_n(body_valid[i+1]),
        .if_read(body_ready[i+1]),
        .if_dout(body_data[i+1])
      );
    end
  endgenerate

  // fifo_almost_full is supplied by relay_station.v.  Keeping this named
  // Tail hierarchy lets the emitted XDC constrain all storage cells to the
  // destination slot.
  (* keep_hierarchy = "yes" *)
  fifo_almost_full #(
    .DATA_WIDTH(DATA_WIDTH),
    .ADDR_WIDTH(REAL_ADDR_WIDTH),
    .DEPTH(REAL_DEPTH),
    .GRACE_PERIOD(GRACE_PERIOD)
  ) TAPA_HS_TAIL (
    .clk(clk),
    .reset(reset),
    .if_full_n(body_ready[BODY_LEVEL]),
    .if_write_ce(1'b1),
    .if_write(body_valid[BODY_LEVEL]),
    .if_din(body_data[BODY_LEVEL]),
    .if_empty_n(if_empty_n),
    .if_read_ce(1'b1),
    .if_read(active_read),
    .if_dout(if_dout)
  );

endmodule

module tapa_hs_pipeline_head_gate #(
  parameter DATA_WIDTH = 32
) (
  output wire                  if_full_n,
  input  wire                  if_write,
  input  wire [DATA_WIDTH-1:0] if_din,
  output wire                  if_empty_n,
  input  wire                  if_read,
  output wire [DATA_WIDTH-1:0] if_dout
);
  assign if_empty_n = if_write & if_read;
  assign if_full_n = if_read;
  assign if_dout = if_din;
endmodule

module tapa_hs_pipeline_head #(
  parameter DATA_WIDTH = 32
) (
  input wire clk,
  output wire                  if_full_n,
  input  wire                  if_write,
  input  wire [DATA_WIDTH-1:0] if_din,
  output wire                  if_empty_n,
  input  wire                  if_read,
  output wire [DATA_WIDTH-1:0] if_dout
);
  (* keep = "true" *) reg                  if_read_reg;
  (* keep = "true" *) reg                  if_write_reg;
  (* keep = "true" *) reg [DATA_WIDTH-1:0] if_din_reg;

  always @(posedge clk) begin
    if_read_reg <= if_read;
    if_write_reg <= if_write;
    if_din_reg <= if_din;
  end

  assign if_full_n = if_read_reg;
  assign if_empty_n = if_write_reg;
  assign if_dout = if_din_reg;
endmodule

module tapa_hs_pipeline_body #(
  parameter DATA_WIDTH = 32
) (
  input wire clk,
  output wire                  if_full_n,
  input  wire                  if_write,
  input  wire [DATA_WIDTH-1:0] if_din,
  output wire                  if_empty_n,
  input  wire                  if_read,
  output wire [DATA_WIDTH-1:0] if_dout
);
  (* keep = "true" *) reg                  if_full_n_reg;
  (* keep = "true" *) reg                  if_empty_n_reg;
  (* keep = "true" *) reg [DATA_WIDTH-1:0] if_dout_reg;

  always @(posedge clk) begin
    if_full_n_reg <= if_read;
    if_empty_n_reg <= if_write;
    if_dout_reg <= if_din;
  end

  assign if_full_n = if_full_n_reg;
  assign if_empty_n = if_empty_n_reg;
  assign if_dout = if_dout_reg;
endmodule

`default_nettype wire
