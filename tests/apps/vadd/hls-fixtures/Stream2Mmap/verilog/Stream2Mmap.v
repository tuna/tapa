// Hand-authored behavioral stand-in for the Vitis HLS output of the
// `Stream2Mmap` lower task of `tests/apps/vadd/vadd.cpp`; see Mmap2Stream.v
// for the fixture rationale and the interface-convention references.
//
// Behavior: while `ap_start` is held, pop n words from `stream_s` and write
// each to `mmap_offset + 4*i` with a single-beat M-AXI write (WSTRB all
// ones). The EOT bit (32) is ignored, matching the kernel's counted loop.
// The unused read channels are tied off; the peek port is never popped.
`timescale 1 ns / 1 ps

module Stream2Mmap (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        stream_s_dout,
        stream_s_empty_n,
        stream_s_read,
        stream_peek_dout,
        stream_peek_empty_n,
        stream_peek_read,
        mmap_offset,
        n,
        m_axi_mmap_AWVALID,
        m_axi_mmap_AWREADY,
        m_axi_mmap_AWADDR,
        m_axi_mmap_AWID,
        m_axi_mmap_AWLEN,
        m_axi_mmap_AWSIZE,
        m_axi_mmap_AWBURST,
        m_axi_mmap_WVALID,
        m_axi_mmap_WREADY,
        m_axi_mmap_WDATA,
        m_axi_mmap_WSTRB,
        m_axi_mmap_WLAST,
        m_axi_mmap_BVALID,
        m_axi_mmap_BREADY,
        m_axi_mmap_BID,
        m_axi_mmap_BRESP,
        m_axi_mmap_ARVALID,
        m_axi_mmap_ARREADY,
        m_axi_mmap_ARADDR,
        m_axi_mmap_ARID,
        m_axi_mmap_ARLEN,
        m_axi_mmap_ARSIZE,
        m_axi_mmap_ARBURST,
        m_axi_mmap_RVALID,
        m_axi_mmap_RREADY,
        m_axi_mmap_RDATA,
        m_axi_mmap_RID,
        m_axi_mmap_RLAST,
        m_axi_mmap_RRESP
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [32:0] stream_s_dout;
input   stream_s_empty_n;
output   stream_s_read;
input  [32:0] stream_peek_dout;
input   stream_peek_empty_n;
output   stream_peek_read;
input  [63:0] mmap_offset;
input  [63:0] n;
output   m_axi_mmap_AWVALID;
input   m_axi_mmap_AWREADY;
output  [63:0] m_axi_mmap_AWADDR;
output   m_axi_mmap_AWID;
output  [7:0] m_axi_mmap_AWLEN;
output  [2:0] m_axi_mmap_AWSIZE;
output  [1:0] m_axi_mmap_AWBURST;
output   m_axi_mmap_WVALID;
input   m_axi_mmap_WREADY;
output  [31:0] m_axi_mmap_WDATA;
output  [3:0] m_axi_mmap_WSTRB;
output   m_axi_mmap_WLAST;
input   m_axi_mmap_BVALID;
output   m_axi_mmap_BREADY;
input   m_axi_mmap_BID;
input  [1:0] m_axi_mmap_BRESP;
output   m_axi_mmap_ARVALID;
input   m_axi_mmap_ARREADY;
output  [63:0] m_axi_mmap_ARADDR;
output   m_axi_mmap_ARID;
output  [7:0] m_axi_mmap_ARLEN;
output  [2:0] m_axi_mmap_ARSIZE;
output  [1:0] m_axi_mmap_ARBURST;
input   m_axi_mmap_RVALID;
output   m_axi_mmap_RREADY;
input  [31:0] m_axi_mmap_RDATA;
input   m_axi_mmap_RID;
input   m_axi_mmap_RLAST;
input  [1:0] m_axi_mmap_RRESP;

localparam ST_IDLE = 3'd0;
localparam ST_POP  = 3'd1;
localparam ST_AW   = 3'd2;
localparam ST_W    = 3'd3;
localparam ST_B    = 3'd4;
localparam ST_DONE = 3'd5;

reg  [2:0]  state;
reg  [63:0] idx;
reg  [31:0] wdata_q;

reg         ap_done_r;
reg         ap_idle_r;
reg         ap_ready_r;
reg         read_r;
reg         awvalid_r;
reg         wvalid_r;
reg         bready_r;

assign ap_done  = ap_done_r;
assign ap_idle  = ap_idle_r;
assign ap_ready = ap_ready_r;

assign stream_s_read    = read_r;
assign stream_peek_read = 1'b0;

assign m_axi_mmap_AWVALID = awvalid_r;
assign m_axi_mmap_AWADDR  = mmap_offset + (idx << 2);
assign m_axi_mmap_AWID    = 1'b0;
assign m_axi_mmap_AWLEN   = 8'd0;
assign m_axi_mmap_AWSIZE  = 3'd2;
assign m_axi_mmap_AWBURST = 2'b01;
assign m_axi_mmap_WVALID  = wvalid_r;
assign m_axi_mmap_WDATA   = wdata_q;
assign m_axi_mmap_WSTRB   = 4'b1111;
assign m_axi_mmap_WLAST   = 1'b1;
assign m_axi_mmap_BREADY  = bready_r;

// This task never reads: tie the read side off.
assign m_axi_mmap_ARVALID = 1'b0;
assign m_axi_mmap_ARADDR  = 64'd0;
assign m_axi_mmap_ARID    = 1'b0;
assign m_axi_mmap_ARLEN   = 8'd0;
assign m_axi_mmap_ARSIZE  = 3'd0;
assign m_axi_mmap_ARBURST = 2'd0;
assign m_axi_mmap_RREADY  = 1'b0;

always @(posedge ap_clk or negedge ap_rst_n) begin
    if (!ap_rst_n) begin
        state      <= ST_IDLE;
        idx        <= 64'd0;
        wdata_q    <= 32'd0;
        ap_done_r  <= 1'b0;
        ap_idle_r  <= 1'b1;
        ap_ready_r <= 1'b0;
        read_r     <= 1'b0;
        awvalid_r  <= 1'b0;
        wvalid_r   <= 1'b0;
        bready_r   <= 1'b0;
    end else begin
        read_r <= 1'b0;
        case (state)
            ST_IDLE: begin
                ap_done_r  <= 1'b0;
                ap_ready_r <= 1'b0;
                if (ap_start) begin
                    ap_idle_r <= 1'b0;
                    idx       <= 64'd0;
                    state     <= (n == 64'd0) ? ST_DONE : ST_POP;
                end
            end
            ST_POP: begin
                if (stream_s_empty_n) begin
                    wdata_q   <= stream_s_dout[31:0];
                    read_r    <= 1'b1;
                    awvalid_r <= 1'b1;
                    state     <= ST_AW;
                end
            end
            ST_AW: begin
                if (m_axi_mmap_AWREADY) begin
                    awvalid_r <= 1'b0;
                    wvalid_r  <= 1'b1;
                    state     <= ST_W;
                end
            end
            ST_W: begin
                if (m_axi_mmap_WREADY) begin
                    wvalid_r <= 1'b0;
                    bready_r <= 1'b1;
                    state    <= ST_B;
                end
            end
            ST_B: begin
                if (m_axi_mmap_BVALID) begin
                    bready_r <= 1'b0;
                    idx      <= idx + 64'd1;
                    if (idx + 64'd1 == n) begin
                        state <= ST_DONE;
                    end else begin
                        state <= ST_POP;
                    end
                end
            end
            ST_DONE: begin
                ap_done_r  <= 1'b1;
                ap_ready_r <= 1'b1;
                if (!ap_start) begin
                    ap_done_r  <= 1'b0;
                    ap_ready_r <= 1'b0;
                    ap_idle_r  <= 1'b1;
                    state      <= ST_IDLE;
                end
            end
            default: state <= ST_IDLE;
        endcase
    end
end

endmodule
