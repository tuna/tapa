`default_nettype none

// Turns `{count, byte address}` requests into `{beats - 1, byte address}`
// pieces for the burst detector, read like a FIFO, one per cycle: pieces of
// at most MaxBurstLen + 1 beats that do not cross a 4 KB boundary. A
// request of one element is one piece of one beat, an element request. A
// request of zero elements is consumed without a piece.
module burst_split #(
  parameter AddrWidth         = 64,
  parameter LenWidth          = 32,
  parameter DataWidthBytesLog = 6,
  parameter BurstLenWidth     = 8,
  parameter MaxBurstLen       = 255
) (
  input wire clk,
  input wire rst,  // active high

  // requests, read like a FIFO
  input  wire [LenWidth+AddrWidth-1:0] req_dout,
  input  wire                          req_empty_n,
  output wire                          req_read,

  // pieces, read like a FIFO
  output wire [BurstLenWidth+AddrWidth-1:0] addr_dout,
  output wire                               addr_empty_n,
  input  wire                               addr_read
);

  localparam [12:0] MaxBeats = MaxBurstLen + 1;

  wire [AddrWidth-1:0] req_addr = req_dout[AddrWidth-1:0];
  wire [LenWidth-1:0]  req_len  = req_dout[LenWidth+AddrWidth-1:AddrWidth];

  reg                 busy;  // `addr` and `left` hold an unfinished request
  reg [AddrWidth-1:0] addr;  // next address of that request
  reg [LenWidth-1:0]  left;  // elements of that request still to send

  wire [AddrWidth-1:0] cur_addr  = busy ? addr : req_addr;
  wire [LenWidth-1:0]  cur_left  = busy ? left : req_len;
  wire [12:0]          page_left = (13'd4096 - {1'b0, cur_addr[11:0]}) >> DataWidthBytesLog;
  wire [12:0]          cap       = page_left < MaxBeats ? page_left : MaxBeats;
  wire [LenWidth-1:0]  beats     = cur_left < cap ? cur_left : cap;

  assign addr_dout    = {beats[BurstLenWidth-1:0] - 1'b1, cur_addr};
  assign addr_empty_n = busy || (req_empty_n && req_len != 0);
  assign req_read     = !busy && req_empty_n && (req_len == 0 || addr_read);

  always @(posedge clk) begin
    if (rst) begin
      busy <= 1'b0;
    end else if (addr_empty_n && addr_read) begin
      busy <= beats != cur_left;
      addr <= cur_addr + (beats << DataWidthBytesLog);
      left <= cur_left - beats;
    end
  end

endmodule  // burst_split

`default_nettype wire
