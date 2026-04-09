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

  reg [DATA_WIDTH:0] buffered_payload;
  reg                buffered_valid;

  assign s_axis_tready = !buffered_valid || m_stream_read;
  assign m_stream_empty_n = buffered_valid || s_axis_tvalid;
  assign m_stream_dout = buffered_valid ? buffered_payload : {s_axis_tlast, s_axis_tdata};

  always @(posedge clk) begin
    if (reset) begin
      buffered_payload <= {(DATA_WIDTH + 1){1'b0}};
      buffered_valid <= 1'b0;
    end else if (buffered_valid) begin
      if (m_stream_read) begin
        if (s_axis_tvalid) begin
          buffered_payload <= {s_axis_tlast, s_axis_tdata};
          buffered_valid <= 1'b1;
        end else begin
          buffered_valid <= 1'b0;
        end
      end
    end else if (s_axis_tvalid && !m_stream_read) begin
      buffered_payload <= {s_axis_tlast, s_axis_tdata};
      buffered_valid <= 1'b1;
    end
  end

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

  reg [DATA_WIDTH:0] axis_payload;
  reg                axis_valid;

  assign s_stream_full_n = !axis_valid || m_axis_tready;
  assign m_axis_tvalid = axis_valid;
  assign {m_axis_tlast, m_axis_tdata} = axis_payload;

  always @(posedge clk) begin
    if (reset) begin
      axis_payload <= {(DATA_WIDTH + 1){1'b0}};
      axis_valid <= 1'b0;
    end else if (axis_valid) begin
      if (m_axis_tready) begin
        if (s_stream_write) begin
          axis_payload <= s_stream_din;
          axis_valid <= 1'b1;
        end else begin
          axis_valid <= 1'b0;
        end
      end
    end else if (s_stream_write) begin
      axis_payload <= s_stream_din;
      axis_valid <= 1'b1;
    end
  end

endmodule

`default_nettype wire
