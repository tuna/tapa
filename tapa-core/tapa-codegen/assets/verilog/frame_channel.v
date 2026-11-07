`default_nettype none

// A stream of frames: BANKS banks of FRAME elements in one memory, handed
// between a producer and a consumer in order. Each side is either a frame
// port (*_VIEW = 1), which acquires a bank, addresses element i of it at
// bank * FRAME + i through a one-port memory interface with read latency 1,
// and releases it, or an element stream (*_VIEW = 0), whose elements the
// channel puts into or takes from consecutive slots of each bank. Banks are
// released in the order they were acquired on each side, so a release token
// carries no data.
module frame_channel #(
  parameter DATA_WIDTH = 32,
  parameter FRAME      = 1024,
  parameter BANKS      = 2,
  parameter ADDR_WIDTH = 18,  // of a frame port's memory interface
  parameter BANK_WIDTH = 8,   // of a frame port's acquire token
  parameter W_VIEW     = 1,
  parameter R_VIEW     = 1,
  parameter MEM_STYLE  = "auto"
) (
  input wire clk,
  input wire reset,

  // producer frame port
  input  wire [ADDR_WIDTH-1:0] w_buf_address0,
  input  wire                  w_buf_ce0,
  input  wire                  w_buf_we0,
  input  wire [DATA_WIDTH-1:0] w_buf_d0,
  output wire [BANK_WIDTH-1:0] w_acquire_dout,
  output wire                  w_acquire_empty_n,
  input  wire                  w_acquire_read,
  output wire                  w_release_full_n,
  input  wire                  w_release_write,

  // producer element stream; its end-of-transmission bit (the top) is unused
  input  wire [DATA_WIDTH:0]   if_din,
  output wire                  if_full_n,
  input  wire                  if_write,

  // consumer frame port
  input  wire [ADDR_WIDTH-1:0] r_buf_address0,
  input  wire                  r_buf_ce0,
  output wire [DATA_WIDTH-1:0] r_buf_q0,
  output wire [BANK_WIDTH-1:0] r_acquire_dout,
  output wire                  r_acquire_empty_n,
  input  wire                  r_acquire_read,
  output wire                  r_release_full_n,
  input  wire                  r_release_write,

  // consumer element stream; it never carries end-of-transmission
  output wire [DATA_WIDTH:0]   if_dout,
  output wire                  if_empty_n,
  input  wire                  if_read
);

localparam DEPTH       = BANKS * FRAME;
localparam MEM_WIDTH   = DEPTH > 1 ? $clog2(DEPTH) : 1;
localparam INDEX_WIDTH = FRAME > 1 ? $clog2(FRAME) : 1;
localparam COUNT_WIDTH = $clog2(BANKS + 1);

(* ram_style = MEM_STYLE *)
reg  [DATA_WIDTH-1:0] mem[0:DEPTH-1];
wire [MEM_WIDTH-1:0]  waddr;
wire                  wen;
wire [DATA_WIDTH-1:0] wdata;
wire [MEM_WIDTH-1:0]  raddr;
wire                  ren;
reg  [DATA_WIDTH-1:0] rdata;

always @(posedge clk) begin
  if (wen) mem[waddr] <= wdata;
  if (ren) rdata <= mem[raddr];
end

// Banks: the producer takes w_bank next, the consumer r_bank; free_banks
// are ready for the producer, full_banks for the consumer.
reg  [BANK_WIDTH-1:0]  w_bank;
reg  [BANK_WIDTH-1:0]  r_bank;
reg  [COUNT_WIDTH-1:0] free_banks;
reg  [COUNT_WIDTH-1:0] full_banks;
wire                   w_ready = free_banks != 0;
wire                   r_ready = full_banks != 0;
wire                   w_acq;
wire                   w_rel;
wire                   r_acq;
wire                   r_rel;

always @(posedge clk) begin
  if (reset) begin
    w_bank     <= {BANK_WIDTH{1'b0}};
    r_bank     <= {BANK_WIDTH{1'b0}};
    free_banks <= BANKS[COUNT_WIDTH-1:0];
    full_banks <= {COUNT_WIDTH{1'b0}};
  end else begin
    if (w_acq) w_bank <= w_bank == BANKS - 1 ? {BANK_WIDTH{1'b0}} : w_bank + 1'b1;
    if (r_acq) r_bank <= r_bank == BANKS - 1 ? {BANK_WIDTH{1'b0}} : r_bank + 1'b1;
    free_banks <= free_banks - w_acq + r_rel;
    full_banks <= full_banks - r_acq + w_rel;
  end
end

generate
  if (W_VIEW) begin : producer_view
    assign waddr             = w_buf_address0[MEM_WIDTH-1:0];
    assign wen               = w_buf_ce0 & w_buf_we0;
    assign wdata             = w_buf_d0;
    assign w_acquire_dout    = w_bank;
    assign w_acquire_empty_n = w_ready;
    assign w_acq             = w_acquire_read & w_ready;
    assign w_release_full_n  = 1'b1;
    assign w_rel             = w_release_write;
    assign if_full_n         = 1'b0;
  end else begin : producer_stream
    reg                   have;
    reg [MEM_WIDTH-1:0]   addr;
    reg [INDEX_WIDTH-1:0] index;
    wire push = have & if_write;
    wire last = push & (index == FRAME - 1);

    assign w_acq             = w_ready & (~have | last);
    assign w_rel             = last;
    assign waddr             = addr;
    assign wen               = push;
    assign wdata             = if_din[DATA_WIDTH-1:0];
    assign if_full_n         = have;
    assign w_acquire_dout    = {BANK_WIDTH{1'b0}};
    assign w_acquire_empty_n = 1'b0;
    assign w_release_full_n  = 1'b0;

    always @(posedge clk) begin
      if (reset) begin
        have  <= 1'b0;
        index <= {INDEX_WIDTH{1'b0}};
      end else begin
        if (w_acq) have <= 1'b1;
        else if (last) have <= 1'b0;
        if (push) index <= last ? {INDEX_WIDTH{1'b0}} : index + 1'b1;
      end
      if (w_acq) addr <= w_bank * FRAME;
      else if (push) addr <= addr + 1'b1;
    end
  end

  if (R_VIEW) begin : consumer_view
    assign raddr             = r_buf_address0[MEM_WIDTH-1:0];
    assign ren               = r_buf_ce0;
    assign r_buf_q0          = rdata;
    assign r_acquire_dout    = r_bank;
    assign r_acquire_empty_n = r_ready;
    assign r_acq             = r_acquire_read & r_ready;
    assign r_release_full_n  = 1'b1;
    assign r_rel             = r_release_write;
    assign if_dout           = {(DATA_WIDTH + 1){1'b0}};
    assign if_empty_n        = 1'b0;
  end else begin : consumer_stream
    // Reads are issued ahead into a two-entry buffer, so the stream moves
    // one element per cycle despite the memory's read latency.
    reg                   have;
    reg [MEM_WIDTH-1:0]   addr;
    reg [INDEX_WIDTH-1:0] index;
    reg                   pending;  // a read issued last cycle: rdata holds it
    reg [DATA_WIDTH-1:0]  head;
    reg [DATA_WIDTH-1:0]  tail;
    reg [1:0]             count;
    wire pop   = if_read & (count != 2'd0);
    wire issue = have & ({1'b0, count} + pending - pop < 3'd2);
    wire last  = issue & (index == FRAME - 1);

    assign r_acq             = r_ready & (~have | last);
    assign r_rel             = last;
    assign raddr             = addr;
    assign ren               = issue;
    assign if_dout           = {1'b0, head};
    assign if_empty_n        = count != 2'd0;
    assign r_buf_q0          = {DATA_WIDTH{1'b0}};
    assign r_acquire_dout    = {BANK_WIDTH{1'b0}};
    assign r_acquire_empty_n = 1'b0;
    assign r_release_full_n  = 1'b0;

    always @(posedge clk) begin
      if (reset) begin
        have    <= 1'b0;
        index   <= {INDEX_WIDTH{1'b0}};
        pending <= 1'b0;
        count   <= 2'd0;
      end else begin
        if (r_acq) have <= 1'b1;
        else if (last) have <= 1'b0;
        if (issue) index <= last ? {INDEX_WIDTH{1'b0}} : index + 1'b1;
        pending <= issue;
        count   <= count + pending - pop;
      end
      if (r_acq) addr <= r_bank * FRAME;
      else if (issue) addr <= addr + 1'b1;
      if (pop) begin
        head <= count == 2'd2 ? tail : rdata;
        if (count == 2'd2) tail <= rdata;
      end else if (pending) begin
        if (count == 2'd0) head <= rdata;
        else tail <= rdata;
      end
    end
  end
endgenerate

endmodule

`default_nettype wire
