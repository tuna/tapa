`default_nettype none

// Detect bursts from a stream of `{beats - 1, address}` pieces: merge
// consecutive pieces into AXI bursts. A piece of one element is an element
// request; a longer one is a piece of a burst request. A burst of element
// requests only is at most max_burst_len + 1 beats, and waits up to
// max_wait_time cycles for its next element. A burst holding a piece of a
// burst request is at most max_request_burst_len + 1 beats, and goes out
// the first cycle no contiguous piece is there to extend it.
module detect_burst #(
  parameter AddrWidth         = 64,
  parameter DataWidthBytesLog = 6,
  parameter WaitTimeWidth     = 4,
  parameter BurstLenWidth     = 8
) (
  input wire clk,
  input wire rst,

  input wire [WaitTimeWidth-1:0] max_wait_time,
  input wire [BurstLenWidth-1:0] max_burst_len,  // 0 disables detection
  input wire [BurstLenWidth-1:0] max_request_burst_len,
  // While hold is high an open burst waits for its next address past
  // max_wait_time: the address is late only because its user is blocked
  // on something that frees itself.
  input wire                     hold,

  input  wire [BurstLenWidth+AddrWidth-1:0] addr_dout,
  input  wire                               addr_empty_n,
  output reg                                addr_read,

  output wire [BurstLenWidth+AddrWidth-1:0] addr_din,
  input  wire                               addr_full_n,
  output wire                               addr_write,

  output wire [BurstLenWidth-1:0] burst_len_0_din,
  input  wire                     burst_len_0_full_n,
  output wire                     burst_len_0_write,

  output wire [BurstLenWidth-1:0] burst_len_1_din,
  input  wire                     burst_len_1_full_n,
  output wire                     burst_len_1_write
);
  // parameter
  localparam NextAddrWidth = AddrWidth - DataWidthBytesLog;
  // AXI burst must not cross a 4KB boundary
  localparam BurstBoundaryLenWidth = $clog2(4096);

  // state
  reg [AddrWidth-1:0]     base_addr;
  reg                     base_valid;
  reg                     eager;  // the open burst holds a burst request's piece
  reg [BurstLenWidth-1:0] burst_len;
  reg [WaitTimeWidth-1:0] wait_time;
  reg [NextAddrWidth-1:0] next_addr;

  // logic
  reg                     write_enable;
  reg [AddrWidth-1:0]     base_addr_next;
  reg                     base_valid_next;
  reg                     eager_next;
  reg [BurstLenWidth-1:0] burst_len_next;
  reg [WaitTimeWidth-1:0] wait_time_next;

  wire [NextAddrWidth-1:0] next_addr_next =
      base_addr_next[AddrWidth-1:DataWidthBytesLog] +
      {{(NextAddrWidth-BurstLenWidth){1'b0}}, burst_len_next} +
      {{(NextAddrWidth-1){1'b0}}, 1'b1};

  assign addr_write = write_enable;
  assign burst_len_0_write = write_enable;
  assign burst_len_1_write = write_enable;
  assign addr_din = {burst_len, base_addr};
  assign burst_len_0_din = burst_len;
  assign burst_len_1_din = burst_len;

  // register the input for timing closure
  reg addr_empty_n_q;
  reg [BurstLenWidth+AddrWidth-1:0] addr_dout_q;
  always @(posedge clk) begin
    if (addr_full_n && burst_len_0_full_n && burst_len_1_full_n) begin
      addr_empty_n_q <= addr_empty_n;
      addr_dout_q <= addr_dout;
    end
  end
  wire [AddrWidth-1:0]     curr_addr = addr_dout_q[AddrWidth-1:0];
  wire [BurstLenWidth-1:0] curr_len  = addr_dout_q[BurstLenWidth+AddrWidth-1:AddrWidth];
  // the open burst extended by the current piece, and whether it fits
  wire [BurstLenWidth:0] merged_len = burst_len + curr_len + 1'b1;
  wire                   merged_fits = eager || curr_len != 0 ?
      merged_len <= max_request_burst_len : burst_len < max_burst_len;

  always @* begin
    // defaults
    addr_read = 1'b0;
    if (!addr_full_n || !burst_len_0_full_n || !burst_len_1_full_n) begin
      addr_read = 1'b0;
    end else if (addr_empty_n) begin
      // read new item if non-empty
      addr_read = 1'b1;
    end else if (base_valid) begin
      addr_read = 1'b0;
    end
  end

  always @* begin
    // defaults
    write_enable = 1'b0;
    base_addr_next = base_addr;
    base_valid_next = base_valid;
    eager_next = eager;
    wait_time_next = wait_time;
    burst_len_next = burst_len;
    if (!addr_full_n || !burst_len_0_full_n || !burst_len_1_full_n) begin
      // output FIFO full, do nothing
    end else if (addr_empty_n_q) begin
      wait_time_next = 0;
      if (!base_valid) begin
        base_addr_next = curr_addr;
        base_valid_next = 1'b1;
        eager_next = curr_len != 0;

        write_enable = 1'b0;
        burst_len_next = curr_len;
      end else begin
        if (next_addr == curr_addr[AddrWidth-1:DataWidthBytesLog] &&
            curr_addr[BurstBoundaryLenWidth-1:0] != {BurstBoundaryLenWidth{1'b0}} &&
            merged_fits) begin
          burst_len_next = merged_len[BurstLenWidth-1:0];
          eager_next = eager || curr_len != 0;

          write_enable = 1'b0;
          base_addr_next = base_addr;
          base_valid_next = base_valid;
        end else begin
          // no more burst detected
          write_enable = 1'b1;
          burst_len_next = curr_len;
          base_addr_next = curr_addr;
          eager_next = curr_len != 0;

          base_valid_next = base_valid;
        end
      end
    end else if (base_valid && eager) begin
      // a burst request does not wait
      write_enable = 1'b1;
      base_valid_next = 1'b0;
      eager_next = 1'b0;
      burst_len_next = 0;
    end else if (base_valid && hold) begin
      // keep the open burst, and its wait time
    end else if (base_valid) begin
      if (wait_time < max_wait_time) begin
        wait_time_next = wait_time + 1;

        write_enable = 1'b0;
        base_addr_next = base_addr;
        base_valid_next = base_valid;
        burst_len_next = burst_len;
      end else begin
        write_enable = 1'b1;
        wait_time_next = 0;
        base_valid_next = 1'b0;
        burst_len_next = 0;

        base_addr_next = base_addr;
      end
    end
  end

  always @(posedge clk) begin
    if (rst) begin
      base_addr <= {AddrWidth{1'b0}};
      base_valid <= 1'd0;
      eager <= 1'b0;
      burst_len <= {BurstLenWidth{1'b0}};
      wait_time <= {WaitTimeWidth{1'b0}};
      next_addr <= {{(NextAddrWidth-1){1'b0}}, 1'b1};
    end else begin
      base_addr <= base_addr_next;
      base_valid <= base_valid_next;
      eager <= eager_next;
      burst_len <= burst_len_next;
      wait_time <= wait_time_next;
      next_addr <= next_addr_next;
    end
  end

endmodule  // detect_burst

`default_nettype wire
