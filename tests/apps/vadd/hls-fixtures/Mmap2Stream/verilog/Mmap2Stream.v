// Hand-authored behavioral stand-in for the Vitis HLS output of the
// `Mmap2Stream` lower task of `tests/apps/vadd/vadd.cpp`, so the
// analyze -> synth -> pack -> cosim chain runs without vendor tools.
// Port spellings follow the conventions pinned by `tapa-protocol`
// (`{base}_s*` streams, `{port}_offset` direct mmap offset, compact
// M-AXI master set) against the real task-graph argument names.
//
// Behavior: while `ap_start` is held, issue one single-beat M-AXI read per
// element (i in 0..n) at `mmap_offset + 4*i` and push each returned word
// onto `stream_s` with the EOT bit (32) tied low -- the kernel never
// closes the stream. The unused write channels are tied off.
`timescale 1 ns / 1 ps

module Mmap2Stream (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
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
        m_axi_mmap_RRESP,
        stream_s_din,
        stream_s_full_n,
        stream_s_write
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
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
output  [32:0] stream_s_din;
input   stream_s_full_n;
output   stream_s_write;

localparam ST_IDLE = 3'd0;
localparam ST_AR   = 3'd1;
localparam ST_R    = 3'd2;
localparam ST_PUSH = 3'd3;
localparam ST_DONE = 3'd4;

reg  [2:0]  state;
reg  [63:0] idx;
reg  [31:0] rdata_q;

reg         ap_done_r;
reg         ap_idle_r;
reg         ap_ready_r;
reg         arvalid_r;
reg         rready_r;
reg  [32:0] din_r;
reg         write_r;

assign ap_done  = ap_done_r;
assign ap_idle  = ap_idle_r;
assign ap_ready = ap_ready_r;

assign m_axi_mmap_ARVALID = arvalid_r;
assign m_axi_mmap_ARADDR  = mmap_offset + (idx << 2);
assign m_axi_mmap_ARID    = 1'b0;
assign m_axi_mmap_ARLEN   = 8'd0;
assign m_axi_mmap_ARSIZE  = 3'd2;
assign m_axi_mmap_ARBURST = 2'b01;
assign m_axi_mmap_RREADY  = rready_r;

// This task never writes back: tie the write side off.
assign m_axi_mmap_AWVALID = 1'b0;
assign m_axi_mmap_AWADDR  = 64'd0;
assign m_axi_mmap_AWID    = 1'b0;
assign m_axi_mmap_AWLEN   = 8'd0;
assign m_axi_mmap_AWSIZE  = 3'd0;
assign m_axi_mmap_AWBURST = 2'd0;
assign m_axi_mmap_WVALID  = 1'b0;
assign m_axi_mmap_WDATA   = 32'd0;
assign m_axi_mmap_WSTRB   = 4'd0;
assign m_axi_mmap_WLAST   = 1'b0;
assign m_axi_mmap_BREADY  = 1'b0;

assign stream_s_din   = din_r;
assign stream_s_write = write_r;

always @(posedge ap_clk or negedge ap_rst_n) begin
    if (!ap_rst_n) begin
        state      <= ST_IDLE;
        idx        <= 64'd0;
        rdata_q    <= 32'd0;
        ap_done_r  <= 1'b0;
        ap_idle_r  <= 1'b1;
        ap_ready_r <= 1'b0;
        arvalid_r  <= 1'b0;
        rready_r   <= 1'b0;
        din_r      <= 33'd0;
        write_r    <= 1'b0;
    end else begin
        write_r <= 1'b0;
        case (state)
            ST_IDLE: begin
                ap_done_r  <= 1'b0;
                ap_ready_r <= 1'b0;
                if (ap_start) begin
                    ap_idle_r <= 1'b0;
                    idx       <= 64'd0;
                    if (n == 64'd0) begin
                        state <= ST_DONE;
                    end else begin
                        arvalid_r <= 1'b1;
                        state     <= ST_AR;
                    end
                end
            end
            ST_AR: begin
                if (m_axi_mmap_ARREADY) begin
                    arvalid_r <= 1'b0;
                    rready_r  <= 1'b1;
                    state     <= ST_R;
                end
            end
            ST_R: begin
                if (m_axi_mmap_RVALID) begin
                    rready_r <= 1'b0;
                    rdata_q  <= m_axi_mmap_RDATA;
                    state    <= ST_PUSH;
                end
            end
            ST_PUSH: begin
                if (stream_s_full_n) begin
                    din_r   <= {1'b0, rdata_q};
                    write_r <= 1'b1;
                    idx     <= idx + 64'd1;
                    if (idx + 64'd1 == n) begin
                        state <= ST_DONE;
                    end else begin
                        arvalid_r <= 1'b1;
                        state     <= ST_AR;
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
