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

  reg [DATA_WIDTH:0] stage0_payload;
  reg                stage0_valid;
  reg [DATA_WIDTH:0] stage1_payload;
  reg                stage1_valid;
  wire [DATA_WIDTH:0] s_axis_payload;

  assign s_axis_payload = {s_axis_tlast, s_axis_tdata};
  assign s_axis_tready = !stage1_valid || m_stream_read;
  assign m_stream_empty_n = stage0_valid || s_axis_tvalid;
  assign m_stream_dout = stage0_valid ? stage0_payload : s_axis_payload;

  always @(posedge clk) begin
    if (reset) begin
      stage0_payload <= {(DATA_WIDTH + 1){1'b0}};
      stage0_valid <= 1'b0;
      stage1_payload <= {(DATA_WIDTH + 1){1'b0}};
      stage1_valid <= 1'b0;
    end else if (stage0_valid) begin
      if (m_stream_read) begin
        if (stage1_valid) begin
          stage0_payload <= stage1_payload;
          stage0_valid <= 1'b1;
          if (s_axis_tvalid) begin
            stage1_payload <= s_axis_payload;
            stage1_valid <= 1'b1;
          end else begin
            stage1_valid <= 1'b0;
          end
        end else if (s_axis_tvalid) begin
          stage0_payload <= s_axis_payload;
          stage0_valid <= 1'b1;
        end else begin
          stage0_valid <= 1'b0;
        end
      end else if (s_axis_tvalid && !stage1_valid) begin
        stage1_payload <= s_axis_payload;
        stage1_valid <= 1'b1;
      end
    end else if (s_axis_tvalid && !m_stream_read) begin
      stage0_payload <= s_axis_payload;
      stage0_valid <= 1'b1;
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

  reg [DATA_WIDTH:0] stage0_payload;
  reg                stage0_valid;
  reg [DATA_WIDTH:0] stage1_payload;
  reg                stage1_valid;

  assign s_stream_full_n = !stage1_valid || m_axis_tready;
  assign m_axis_tvalid = stage0_valid || s_stream_write;
  assign {m_axis_tlast, m_axis_tdata} = stage0_valid ? stage0_payload : s_stream_din;

  always @(posedge clk) begin
    if (reset) begin
      stage0_payload <= {(DATA_WIDTH + 1){1'b0}};
      stage0_valid <= 1'b0;
      stage1_payload <= {(DATA_WIDTH + 1){1'b0}};
      stage1_valid <= 1'b0;
    end else if (stage0_valid) begin
      if (m_axis_tready) begin
        if (stage1_valid) begin
          stage0_payload <= stage1_payload;
          stage0_valid <= 1'b1;
          if (s_stream_write) begin
            stage1_payload <= s_stream_din;
            stage1_valid <= 1'b1;
          end else begin
            stage1_valid <= 1'b0;
          end
        end else if (s_stream_write) begin
          stage0_payload <= s_stream_din;
          stage0_valid <= 1'b1;
        end else begin
          stage0_valid <= 1'b0;
        end
      end else if (s_stream_write && !stage1_valid) begin
        stage1_payload <= s_stream_din;
        stage1_valid <= 1'b1;
      end
    end else if (s_stream_write && !m_axis_tready) begin
      stage0_payload <= s_stream_din;
      stage0_valid <= 1'b1;
    end
  end

endmodule

`default_nettype wire
