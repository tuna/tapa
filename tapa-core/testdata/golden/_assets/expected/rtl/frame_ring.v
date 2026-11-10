`default_nettype none

// A frame argument of the top carried by a ring of SLOTS frames in device
// memory at `base`, for a host that moves frames through memory (XRT has
// no host streams). The host and the ring hand slots over through two
// frame counters, each counting modulo 2^32 and never reset by a run: the
// host's, `host_frames`, and the ring's, `kernel_frames`. Frame f sits in
// slot f % SLOTS.
//
// INPUT = 1: the host writes frame f into its slot, then raises
// host_frames past f. The ring requests the frame in one burst request and
// writes its elements into the argument's channel (`out_*`); once the last
// one is in, it raises kernel_frames, freeing the slot.
//
// INPUT = 0: the ring reads the elements of each frame from the argument's
// channel (`in_*`). With a frame's first element there and a free slot
// (fewer than SLOTS frames written and not yet taken by the host), it
// requests the frame's write, writes the elements, and raises
// kernel_frames once memory has acknowledged them all; the host then reads
// the frame and raises host_frames past it. `drained` holds while every
// frame released into the channel (one `released` pulse each) is in
// memory: the top is done only then.
//
// The ring follows host_frames only forward (by less than 2^31): XRT
// writes every argument register when it starts a run, host_frames among
// them, with the count the host set before the start, which a frame the
// host announced since has passed.
//
// Requests and data go through an async_mmap (`{count, byte address}`
// requests). A frame the kernel has not taken when a run ends stays in the
// ring for the next one, as it would in a stream.
module frame_ring #(
  parameter DATA_WIDTH = 32,    // of an element, a power of two of bytes
  parameter FRAME      = 1024,  // elements in a frame
  parameter SLOTS      = 4,
  parameter INPUT      = 1,
  parameter ADDR_WIDTH = 64,
  parameter LEN_WIDTH  = 32     // of a request's element count
) (
  input wire clk,
  input wire rst,  // active high

  input  wire [ADDR_WIDTH-1:0] base,
  input  wire [31:0]           host_frames,
  output wire [31:0]           kernel_frames,
  output wire [63:0]           shape,  // {INPUT, SLOTS, FRAME}, for the host

  // INPUT: the elements of each frame, into the argument's channel
  output wire [DATA_WIDTH:0] out_din,
  input  wire                out_full_n,
  output wire                out_write,

  // !INPUT: the elements of each frame, from the argument's channel
  input  wire [DATA_WIDTH:0] in_dout,
  input  wire                in_empty_n,
  output wire                in_read,
  input  wire                released,
  output wire                drained,

  // the async_mmap of the ring's memory
  output wire [LEN_WIDTH+ADDR_WIDTH-1:0] read_addr_din,
  input  wire                            read_addr_full_n,
  output wire                            read_addr_write,
  input  wire [DATA_WIDTH-1:0]           read_data_dout,
  input  wire                            read_data_empty_n,
  output wire                            read_data_read,
  output wire [LEN_WIDTH+ADDR_WIDTH-1:0] write_addr_din,
  input  wire                            write_addr_full_n,
  output wire                            write_addr_write,
  output wire [DATA_WIDTH:0]             write_data_din,  // with a clear EoT bit
  input  wire                            write_data_full_n,
  output wire                            write_data_write,
  input  wire [7:0]                      write_resp_dout,
  input  wire                            write_resp_empty_n,
  output wire                            write_resp_read
);

localparam [ADDR_WIDTH-1:0] FRAME_BYTES = FRAME * (DATA_WIDTH / 8);
localparam [ADDR_WIDTH-1:0] RING_BYTES  = FRAME_BYTES * SLOTS;
localparam [LEN_WIDTH-1:0]  FRAME_LEN   = FRAME;
localparam [31:0]           FRAME_WORD  = FRAME;
localparam [31:0]           SLOTS_WORD  = SLOTS;
localparam INDEX_WIDTH = FRAME > 1 ? $clog2(FRAME) : 1;
// The index of a frame's last element.
localparam [INDEX_WIDTH-1:0] LAST = FRAME_WORD[INDEX_WIDTH-1:0] - 1'b1;

assign shape = {INPUT != 0, SLOTS_WORD[30:0], FRAME_WORD};

// The offset of the slot after `offset`.
function [ADDR_WIDTH-1:0] next_slot(input [ADDR_WIDTH-1:0] offset);
  next_slot = offset + FRAME_BYTES == RING_BYTES ? {ADDR_WIDTH{1'b0}}
                                                 : offset + FRAME_BYTES;
endfunction

reg [ADDR_WIDTH-1:0]  offset;  // of the slot of the next frame to request
reg [INDEX_WIDTH-1:0] index;   // of the element moving next in its frame
reg [31:0]            frames;  // kernel_frames
reg [31:0]            host;    // host_frames, as it last moved forward
wire [31:0]           ahead = host_frames - host;

assign kernel_frames = frames;

always @(posedge clk) begin
  if (rst) host <= 32'd0;
  else if (ahead != 32'd0 && !ahead[31]) host <= host_frames;
end

generate
  if (INPUT) begin : reads
    reg [31:0] requested;  // frames requested from memory
    wire request = host != requested && read_addr_full_n;
    wire move    = read_data_empty_n && out_full_n;
    wire last    = move && index == LAST;

    assign read_addr_din   = {FRAME_LEN, base + offset};
    assign read_addr_write = request;
    assign read_data_read  = move;
    assign out_din         = {1'b0, read_data_dout};
    assign out_write       = move;

    assign write_addr_din   = {(LEN_WIDTH + ADDR_WIDTH){1'b0}};
    assign write_addr_write = 1'b0;
    assign write_data_din   = {(DATA_WIDTH + 1){1'b0}};
    assign write_data_write = 1'b0;
    assign write_resp_read  = 1'b0;
    assign in_read          = 1'b0;
    assign drained          = 1'b1;

    always @(posedge clk) begin
      if (rst) begin
        requested <= 32'd0;
        offset    <= {ADDR_WIDTH{1'b0}};
        index     <= {INDEX_WIDTH{1'b0}};
        frames    <= 32'd0;
      end else begin
        if (request) begin
          requested <= requested + 32'd1;
          offset    <= next_slot(offset);
        end
        if (move) index <= last ? {INDEX_WIDTH{1'b0}} : index + 1'b1;
        if (last) frames <= frames + 32'd1;
      end
    end
  end else begin : writes
    reg [31:0] opened;    // frames whose write was requested
    reg [31:0] released_frames;
    reg        open;      // the frame moving now has its write requested
    reg [31:0] acked;     // elements memory acknowledged, over all frames
    reg [31:0] frame_end; // acked once the oldest unacknowledged frame is in
    wire [31:0] lead = acked - frame_end;
    // A frame's write is requested only with its first element there: a
    // request outliving its data could land in a later process's memory.
    wire start = !open && in_empty_n && opened - host < SLOTS_WORD &&
                 write_addr_full_n;
    wire move  = open && in_empty_n && write_data_full_n;
    wire last  = move && index == LAST;

    assign write_addr_din   = {FRAME_LEN, base + offset};
    assign write_addr_write = start;
    assign write_data_din   = {1'b0, in_dout[DATA_WIDTH-1:0]};
    assign write_data_write = move;
    assign write_resp_read  = write_resp_empty_n;
    assign in_read          = move;
    assign drained          = released_frames == frames;

    assign read_addr_din   = {(LEN_WIDTH + ADDR_WIDTH){1'b0}};
    assign read_addr_write = 1'b0;
    assign read_data_read  = 1'b0;
    assign out_din         = {(DATA_WIDTH + 1){1'b0}};
    assign out_write       = 1'b0;

    always @(posedge clk) begin
      if (rst) begin
        opened          <= 32'd0;
        released_frames <= 32'd0;
        open            <= 1'b0;
        acked           <= 32'd0;
        frame_end       <= FRAME_WORD;
        offset          <= {ADDR_WIDTH{1'b0}};
        index           <= {INDEX_WIDTH{1'b0}};
        frames          <= 32'd0;
      end else begin
        if (start) begin
          opened <= opened + 32'd1;
          offset <= next_slot(offset);
          open   <= 1'b1;
        end else if (last) begin
          open <= 1'b0;
        end
        if (move) index <= last ? {INDEX_WIDTH{1'b0}} : index + 1'b1;
        if (released) released_frames <= released_frames + 32'd1;
        if (write_resp_empty_n) acked <= acked + {24'd0, write_resp_dout} + 32'd1;
        // One frame a cycle: a response may complete several small ones.
        if (!lead[31]) begin
          frames    <= frames + 32'd1;
          frame_end <= frame_end + FRAME_WORD;
        end
      end
    end
  end
endgenerate

endmodule

`default_nettype wire
