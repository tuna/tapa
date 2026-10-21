// Hand-authored stand-in for the Vitis HLS shell of the `VecAdd` upper task
// of `tests/apps/vadd/vadd.cpp`; see Mmap2Stream.v for the fixture
// rationale. Codegen discards the upper-task body and rebuilds the port
// surface (M-AXI masters included) from the task graph, so only the header
// matters: the ap_ctrl handshake plus one direct-offset scalar per mmap
// argument and the scalar `n`, with no `s_axi_control_*` slave -- its
// absence is what selects the HLS-flow top-level shape
// (`top_instantiates_control_s_axi == false`).
`timescale 1 ns / 1 ps

module VecAdd (
        ap_clk,
        ap_rst_n,
        ap_start,
        ap_done,
        ap_idle,
        ap_ready,
        a_offset,
        b_offset,
        c_offset,
        n
);

input   ap_clk;
input   ap_rst_n;
input   ap_start;
output   ap_done;
output   ap_idle;
output   ap_ready;
input  [63:0] a_offset;
input  [63:0] b_offset;
input  [63:0] c_offset;
input  [63:0] n;

reg ap_done;
reg ap_idle;
reg ap_ready;

endmodule
