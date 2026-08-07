`timescale 1ns/1ps

// Port-exposing wrapper around both AXIS adapters. Stimulus and checks live
// in axis_adapter_smoke_main.cpp: driving the clock from C++ keeps the model
// free of delay controls, so Verilator builds it without --timing and thus
// without a C++20 compiler (the CI image is bionic with gcc-7).
module tb (
    input  wire       clk,
    input  wire       reset,

    input  wire [7:0] s_axis_tdata,
    input  wire       s_axis_tvalid,
    output wire       s_axis_tready,
    input  wire       s_axis_tlast,
    output wire [8:0] m_stream_dout,
    output wire       m_stream_empty_n,
    input  wire       m_stream_read,

    input  wire [8:0] s_stream_din,
    output wire       s_stream_full_n,
    input  wire       s_stream_write,
    output wire [7:0] m_axis_tdata,
    output wire       m_axis_tvalid,
    input  wire       m_axis_tready,
    output wire       m_axis_tlast
);

  axis_to_stream_adapter #(.DATA_WIDTH(8)) axis_to_stream (
    .clk(clk),
    .reset(reset),
    .s_axis_tdata(s_axis_tdata),
    .s_axis_tvalid(s_axis_tvalid),
    .s_axis_tready(s_axis_tready),
    .s_axis_tlast(s_axis_tlast),
    .m_stream_dout(m_stream_dout),
    .m_stream_empty_n(m_stream_empty_n),
    .m_stream_read(m_stream_read)
  );

  stream_to_axis_adapter #(.DATA_WIDTH(8)) stream_to_axis (
    .clk(clk),
    .reset(reset),
    .s_stream_din(s_stream_din),
    .s_stream_full_n(s_stream_full_n),
    .s_stream_write(s_stream_write),
    .m_axis_tdata(m_axis_tdata),
    .m_axis_tvalid(m_axis_tvalid),
    .m_axis_tready(m_axis_tready),
    .m_axis_tlast(m_axis_tlast)
  );

endmodule
