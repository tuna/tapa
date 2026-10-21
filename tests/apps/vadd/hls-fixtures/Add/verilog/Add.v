// Hand-authored behavioral stand-in for the Vitis HLS output of the `Add`
// lower task of `tests/apps/vadd/vadd.cpp`; see Mmap2Stream.v for the
// fixture rationale and the interface-convention references.
//
// Behavior: while `ap_start` is held, pop one word from `a_s` and one from
// `b_s` per iteration and push their sum onto `c_s`, n times. The sum goes
// through the same `Add_fadd_32ns_32ns_32_7_full_dsp_1_ip` AXIS stub real
// HLS output instantiates; frt-cosim replaces that stub with its DPI
// behavioral model (fp32_add, latency from the shipped `_ip.tcl`), so this
// fixture contains no FP arithmetic of its own. EOT bits (32) pass through
// tied low, matching the kernel, which never closes its streams. The peek
// ports are never popped.
`timescale 1 ns / 1 ps

module Add (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        a_s_dout,
        a_s_empty_n,
        a_s_read,
        a_peek_dout,
        a_peek_empty_n,
        a_peek_read,
        b_s_dout,
        b_s_empty_n,
        b_s_read,
        b_peek_dout,
        b_peek_empty_n,
        b_peek_read,
        c_s_din,
        c_s_full_n,
        c_s_write,
        n
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [32:0] a_s_dout;
input   a_s_empty_n;
output   a_s_read;
input  [32:0] a_peek_dout;
input   a_peek_empty_n;
output   a_peek_read;
input  [32:0] b_s_dout;
input   b_s_empty_n;
output   b_s_read;
input  [32:0] b_peek_dout;
input   b_peek_empty_n;
output   b_peek_read;
output  [32:0] c_s_din;
input   c_s_full_n;
output   c_s_write;
input  [63:0] n;

localparam ST_IDLE  = 3'd0;
localparam ST_POP_A = 3'd1;
localparam ST_POP_B = 3'd2;
localparam ST_FEED  = 3'd3;
localparam ST_WAIT  = 3'd4;
localparam ST_PUSH  = 3'd5;
localparam ST_DONE  = 3'd6;

reg  [2:0]  state;
reg  [63:0] idx;
reg  [31:0] a_q;
reg  [31:0] b_q;

reg         ap_done_r;
reg         ap_idle_r;
reg         ap_ready_r;
reg         a_read_r;
reg         b_read_r;
reg  [32:0] din_r;
reg         write_r;
reg         feed_r;

wire [31:0] fadd_result;
wire        fadd_result_valid;

assign ap_done  = ap_done_r;
assign ap_idle  = ap_idle_r;
assign ap_ready = ap_ready_r;

assign a_s_read    = a_read_r;
assign b_s_read    = b_read_r;
assign a_peek_read = 1'b0;
assign b_peek_read = 1'b0;
assign c_s_din     = din_r;
assign c_s_write   = write_r;

// One addition in flight at a time, so the NonBlocking AXIS pair below
// never needs tready handling: operands are offered for exactly one cycle
// (ST_FEED) and the result is collected when its tvalid arrives (ST_WAIT).
Add_fadd_32ns_32ns_32_7_full_dsp_1_ip fadd_ip (
    .aclk                  (ap_clk),
    .aclken                (1'b1),
    .s_axis_a_tvalid       (feed_r),
    .s_axis_a_tdata        (a_q),
    .s_axis_b_tvalid       (feed_r),
    .s_axis_b_tdata        (b_q),
    .m_axis_result_tvalid  (fadd_result_valid),
    .m_axis_result_tdata   (fadd_result)
);

always @(posedge ap_clk or negedge ap_rst_n) begin
    if (!ap_rst_n) begin
        state      <= ST_IDLE;
        idx        <= 64'd0;
        a_q        <= 32'd0;
        b_q        <= 32'd0;
        ap_done_r  <= 1'b0;
        ap_idle_r  <= 1'b1;
        ap_ready_r <= 1'b0;
        a_read_r   <= 1'b0;
        b_read_r   <= 1'b0;
        din_r      <= 33'd0;
        write_r    <= 1'b0;
        feed_r     <= 1'b0;
    end else begin
        a_read_r <= 1'b0;
        b_read_r <= 1'b0;
        write_r  <= 1'b0;
        feed_r   <= 1'b0;
        case (state)
            ST_IDLE: begin
                ap_done_r  <= 1'b0;
                ap_ready_r <= 1'b0;
                if (ap_start) begin
                    ap_idle_r <= 1'b0;
                    idx       <= 64'd0;
                    state     <= (n == 64'd0) ? ST_DONE : ST_POP_A;
                end
            end
            ST_POP_A: begin
                if (a_s_empty_n) begin
                    a_q      <= a_s_dout[31:0];
                    a_read_r <= 1'b1;
                    state    <= ST_POP_B;
                end
            end
            ST_POP_B: begin
                if (b_s_empty_n) begin
                    b_q      <= b_s_dout[31:0];
                    b_read_r <= 1'b1;
                    feed_r   <= 1'b1;
                    state    <= ST_WAIT;
                end
            end
            ST_WAIT: begin
                if (fadd_result_valid) begin
                    din_r <= {1'b0, fadd_result};
                    state <= ST_PUSH;
                end
            end
            ST_PUSH: begin
                if (c_s_full_n) begin
                    write_r <= 1'b1;
                    idx     <= idx + 64'd1;
                    if (idx + 64'd1 == n) begin
                        state <= ST_DONE;
                    end else begin
                        state <= ST_POP_A;
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
