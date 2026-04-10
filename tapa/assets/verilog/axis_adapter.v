// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

`default_nettype none

module axis_to_stream_adapter #(
  parameter DATA_WIDTH = 32
) (
  input  wire                  clk,
  input  wire                  reset,
  input  wire [DATA_WIDTH-1:0] s_axis_tdata,
  input  wire                  s_axis_tvalid,
  output wire                  s_axis_tready,
  input  wire                  s_axis_tlast,

  output wire [DATA_WIDTH:0]   m_stream_dout,
  output wire                  m_stream_empty_n,
  input  wire                  m_stream_read
);

  fifo #(
    .DATA_WIDTH(DATA_WIDTH + 1),
    .ADDR_WIDTH(1),
    .DEPTH(2)
  ) fifo_unit (
    .clk        (clk),
    .reset      (reset),
    .if_full_n  (s_axis_tready),
    .if_write_ce(1'b1),
    .if_write   (s_axis_tvalid),
    .if_din     ({s_axis_tlast, s_axis_tdata}),
    .if_empty_n (m_stream_empty_n),
    .if_read_ce (1'b1),
    .if_read    (m_stream_read),
    .if_dout    (m_stream_dout)
  );

endmodule

module stream_to_axis_adapter #(
  parameter DATA_WIDTH = 32
) (
  input  wire                  clk,
  input  wire                  reset,
  input  wire [DATA_WIDTH:0]   s_stream_din,
  output wire                  s_stream_full_n,
  input  wire                  s_stream_write,

  output wire [DATA_WIDTH-1:0] m_axis_tdata,
  output wire                  m_axis_tvalid,
  input  wire                  m_axis_tready,
  output wire                  m_axis_tlast
);

  wire [DATA_WIDTH:0] axis_payload;

  fifo #(
    .DATA_WIDTH(DATA_WIDTH + 1),
    .ADDR_WIDTH(1),
    .DEPTH(2)
  ) fifo_unit (
    .clk        (clk),
    .reset      (reset),
    .if_full_n  (s_stream_full_n),
    .if_write_ce(1'b1),
    .if_write   (s_stream_write),
    .if_din     (s_stream_din),
    .if_empty_n (m_axis_tvalid),
    .if_read_ce (1'b1),
    .if_read    (m_axis_tready),
    .if_dout    (axis_payload)
  );

  assign {m_axis_tlast, m_axis_tdata} = axis_payload;

endmodule

`default_nettype wire
