`default_nettype none

// The end of a run on an output ring of an AI Engine top's PL kernel (see
// frame_ring.v). The AI Engine kernel writing the ring's frames sends, once
// its task returns, how many frames it released that run: one word on
// `count_*`. A run ends once its count is in and the ring has written that
// many frames to memory (`kernel_frames`): `ended` then holds.
//
// The counts are summed, as the ring counts its frames, from reset: a run's
// count may arrive before the kernel starts that run, since the graph runs
// first, so runs are counted as the kernel accepts `start`.
module ring_count (
  input  wire        clk,
  input  wire        rst,  // active high

  input  wire        start,          // the kernel accepts a run this cycle
  input  wire [31:0] kernel_frames,  // the ring's frames in memory

  input  wire [32:0] count_dout,     // with a clear EoT bit
  input  wire        count_empty_n,
  output wire        count_read,

  output wire        ended
);

reg [31:0] runs;      // runs the kernel started
reg [31:0] counts;    // counts received
reg [31:0] expected;  // frames the received counts add up to

assign count_read = count_empty_n;
assign ended = counts == runs && kernel_frames == expected;

always @(posedge clk) begin
  if (rst) begin
    runs     <= 32'd0;
    counts   <= 32'd0;
    expected <= 32'd0;
  end else begin
    if (start) runs <= runs + 32'd1;
    if (count_empty_n) begin
      counts   <= counts + 32'd1;
      expected <= expected + count_dout[31:0];
    end
  end
end

endmodule

`default_nettype wire
