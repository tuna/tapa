`timescale 1ns/1ps

module tb;
  reg clk = 1'b0;
  always #1 clk = ~clk;

  reg reset = 1'b1;

  reg  [7:0] s_axis_tdata = 8'h00;
  reg        s_axis_tvalid = 1'b0;
  wire       s_axis_tready;
  reg        s_axis_tlast = 1'b0;
  wire [8:0] m_stream_dout;
  wire       m_stream_empty_n;
  reg        m_stream_read = 1'b0;

  reg  [8:0] s_stream_din = 9'h000;
  wire       s_stream_full_n;
  reg        s_stream_write = 1'b0;
  wire [7:0] m_axis_tdata;
  wire       m_axis_tvalid;
  reg        m_axis_tready = 1'b0;
  wire       m_axis_tlast;

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

  task automatic check(input bit cond, input string msg);
    if (!cond) begin
      $display("FAIL: %s", msg);
      $fatal(1);
    end
  endtask

  initial begin
    repeat (2) @(posedge clk);
    reset = 1'b0;

    @(negedge clk);
    s_stream_din = 9'h155;
    s_stream_write = 1'b1;
    m_axis_tready = 1'b0;
    #0.1;
    check(m_axis_tvalid === 1'b0, "stream_to_axis is registered");
    @(posedge clk);
    @(negedge clk);
    #0.1;
    check(m_axis_tvalid === 1'b1, "stream_to_axis first beat visible");
    check(
        {m_axis_tlast, m_axis_tdata} === 9'h155,
        "stream_to_axis first beat payload"
    );
    s_stream_write = 1'b0;

    m_axis_tready = 1'b1;
    @(posedge clk);
    @(negedge clk);
    #0.1;
    check(m_axis_tvalid === 1'b0, "stream_to_axis drained");

    s_axis_tdata = 8'h11;
    s_axis_tlast = 1'b1;
    s_axis_tvalid = 1'b1;
    m_stream_read = 1'b0;
    #0.1;
    check(m_stream_empty_n === 1'b0, "axis_to_stream is registered");

    @(posedge clk);
    @(negedge clk);
    #0.1;
    check(m_stream_empty_n === 1'b1, "axis_to_stream first beat visible");
    check(m_stream_dout === 9'h111, "axis_to_stream first beat payload");

    m_stream_read = 1'b1;
    s_axis_tvalid = 1'b0;
    @(posedge clk);
    @(negedge clk);
    #0.1;
    check(m_stream_empty_n === 1'b0, "axis_to_stream drained");

    $finish;
  end
endmodule
