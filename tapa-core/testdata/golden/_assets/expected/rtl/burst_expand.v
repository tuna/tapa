`default_nettype none

// Turns `{count, byte address}` requests into one element address per
// cycle, for a burst detector that reads them like a FIFO. A request of zero
// elements is consumed without an address; the next request's first address
// follows the last one of the current request without a bubble.
module burst_expand #(
  parameter AddrWidth         = 64,
  parameter LenWidth          = 32,
  parameter DataWidthBytesLog = 6
) (
  input wire clk,
  input wire rst,  // active high

  // requests, read like a FIFO
  input  wire [LenWidth+AddrWidth-1:0] req_dout,
  input  wire                          req_empty_n,
  output wire                          req_read,

  // element addresses, read like a FIFO
  output wire [AddrWidth-1:0] addr_dout,
  output wire                 addr_empty_n,
  input  wire                 addr_read
);

  wire [AddrWidth-1:0] req_addr = req_dout[AddrWidth-1:0];
  wire [LenWidth-1:0]  req_len  = req_dout[LenWidth+AddrWidth-1:AddrWidth];
  wire [AddrWidth-1:0] step     = {{(AddrWidth-1){1'b0}}, 1'b1} << DataWidthBytesLog;

  reg                 busy;  // `addr` and `left` hold an unfinished request
  reg [AddrWidth-1:0] addr;  // next address of that request
  reg [LenWidth-1:0]  left;  // addresses of that request still to emit

  assign addr_dout    = busy ? addr : req_addr;
  assign addr_empty_n = busy || (req_empty_n && req_len != 0);
  assign req_read     = !busy && req_empty_n && (req_len == 0 || addr_read);

  always @(posedge clk) begin
    if (rst) begin
      busy <= 1'b0;
    end else if (busy) begin
      if (addr_read) begin
        busy <= left != 1;
        addr <= addr + step;
        left <= left - 1;
      end
    end else if (req_empty_n && req_len > 1 && addr_read) begin
      busy <= 1'b1;
      addr <= req_addr + step;
      left <= req_len - 1;
    end
  end

endmodule  // burst_expand

`default_nettype wire
