__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import logging
from collections.abc import Sequence

from tapa.cosim.common import AXI, Arg
from tapa.cosim.render import (
    render_axi_ram_inst,
    render_axi_ram_module,
    render_hls_test_signals,
    render_testbench_begin,
    render_testbench_end,
    render_vitis_test_signals,
)

_logger = logging.getLogger().getChild(__name__)


def get_axi_ram_inst(axi_obj: AXI) -> str:
    # FIXME: test if using addr_width = 64 will cause problem in simulation
    return render_axi_ram_inst(axi_obj)


def get_s_axi_control() -> str:
    return """
  parameter C_S_AXI_CONTROL_DATA_WIDTH = 32;
  parameter C_S_AXI_CONTROL_ADDR_WIDTH = 64;
  parameter C_S_AXI_DATA_WIDTH = 32;
  parameter C_S_AXI_CONTROL_WSTRB_WIDTH = 32 / 8;
  parameter C_S_AXI_WSTRB_WIDTH = 32 / 8;

  wire                                    s_axi_control_awvalid;
  wire                                    s_axi_control_awready;
  wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0]   s_axi_control_awaddr;
  wire                                    s_axi_control_wvalid;
  wire                                    s_axi_control_wready;
  wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0]   s_axi_control_wdata;
  wire [C_S_AXI_CONTROL_WSTRB_WIDTH-1:0]  s_axi_control_wstrb;
  reg                                     s_axi_control_arvalid = 0;
  wire                                    s_axi_control_arready;
  wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0]   s_axi_control_araddr;
  wire                                    s_axi_control_rvalid;
  wire                                    s_axi_control_rready;
  wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0]   s_axi_control_rdata;
  wire [1:0]                              s_axi_control_rresp;
  wire                                    s_axi_control_bvalid;
  wire                                    s_axi_control_bready;
  wire [1:0]                              s_axi_control_bresp;

  // use a large FIFO to buffer the command to s_axi_control
  // in this way we don't need to worry about flow control
  reg [63:0] s_axi_aw_din = 0;
  reg        s_axi_aw_write = 0;

  fifo_srl #(
    .DATA_WIDTH(64),
    .ADDR_WIDTH(6),
    .DEPTH(64)
  ) s_axi_aw(
    .clk(ap_clk),
    .reset(~ap_rst_n),

    // write
    .if_full_n   (),
    .if_write    (s_axi_aw_write),
    .if_din      (s_axi_aw_din  ),

    // read
    .if_empty_n  (s_axi_control_awvalid),
    .if_read     (s_axi_control_awready),
    .if_dout     (s_axi_control_awaddr),

    .if_write_ce (1'b1),
    .if_read_ce  (1'b1)

  );

  reg [511:0] s_axi_w_din = 0;
  reg         s_axi_w_write = 0;

  fifo_srl #(
    .DATA_WIDTH(32),
    .ADDR_WIDTH(6),
    .DEPTH(64)
  ) s_axi_w(
    .clk(ap_clk),
    .reset(~ap_rst_n),

    // write
    .if_full_n   (),
    .if_write    (s_axi_w_write),
    .if_din      (s_axi_w_din  ),

    // read
    .if_empty_n  (s_axi_control_wvalid),
    .if_read     (s_axi_control_wready),
    .if_dout     (s_axi_control_wdata),

    .if_write_ce (1'b1),
    .if_read_ce  (1'b1)

  );
"""


def get_axis(args: Sequence[Arg]) -> str:
    lines = [get_stream_typedef(args)]
    axis_args = [arg for arg in args if arg.is_stream]
    for arg in axis_args:
        lines.append(
            f"""
  // data ports connected to DUT
  packed_uint{arg.port.data_width}_t axis_{arg.name}_tdata;
  logic axis_{arg.name}_tlast;

  // data ports connected to testbench
  unpacked_uint{arg.port.data_width + 1}_t axis_{arg.name}_tdata_unpacked;
  unpacked_uint{arg.port.data_width + 1}_t axis_{arg.name}_tdata_unpacked_next;

  logic axis_{arg.name}_tvalid = 0;
  logic axis_{arg.name}_tready = 0;
  logic axis_{arg.name}_tvalid_next = 0;
  logic axis_{arg.name}_tready_next = 0;
"""
        )
    return "\n".join(lines)


def get_fifo(args: Sequence[Arg]) -> str:
    lines = [get_stream_typedef(args)]
    fifo_args = [arg for arg in args if arg.is_stream]
    for arg in fifo_args:
        lines.append(
            f"""
  // data ports connected to DUT
  packed_uint{arg.port.data_width + 1}_t fifo_{arg.qualified_name}_data;

  // data ports connected to testbench
  unpacked_uint{arg.port.data_width + 1}_t fifo_{arg.qualified_name}_data_unpacked;
  unpacked_uint{arg.port.data_width + 1}_t fifo_{arg.qualified_name}_data_unpacked_next;

  logic fifo_{arg.qualified_name}_valid = 0;
  logic fifo_{arg.qualified_name}_ready = 0;
  logic fifo_{arg.qualified_name}_valid_next = 0;
  logic fifo_{arg.qualified_name}_ready_next = 0;
"""
        )
    return "\n".join(lines)


def get_stream_typedef(args: Sequence[Arg]) -> str:
    stream_args = [arg for arg in args if arg.is_stream]

    # create type alias for widths used for axis
    widths = set()
    for arg in stream_args:
        widths |= {
            arg.port.data_width,
            arg.port.data_width + 1,  # for eot
        }

    lines = []
    # list comprehension is only more readable when short
    # ruff: noqa: PERF401
    for width in widths:
        lines.append(
            f"""
  typedef logic unpacked_uint{width}_t[{width - 1}:0];
  typedef logic [{width - 1}:0] packed_uint{width}_t;
"""
        )

    return "\n".join(lines)


def get_m_axi_connections(arg_name: str) -> str:
    return f"""
    .m_axi_{arg_name}_ARADDR  (axi_{arg_name}_araddr ),
    .m_axi_{arg_name}_ARBURST (axi_{arg_name}_arburst),
    .m_axi_{arg_name}_ARCACHE (axi_{arg_name}_arcache),
    .m_axi_{arg_name}_ARID    (axi_{arg_name}_arid   ),
    .m_axi_{arg_name}_ARLEN   (axi_{arg_name}_arlen  ),
    .m_axi_{arg_name}_ARLOCK  (axi_{arg_name}_arlock ),
    .m_axi_{arg_name}_ARPROT  (axi_{arg_name}_arprot ),
    .m_axi_{arg_name}_ARQOS   (axi_{arg_name}_arqos  ),
    .m_axi_{arg_name}_ARREADY (axi_{arg_name}_arready),
    .m_axi_{arg_name}_ARSIZE  (axi_{arg_name}_arsize ),
    .m_axi_{arg_name}_ARVALID (axi_{arg_name}_arvalid),
    .m_axi_{arg_name}_AWADDR  (axi_{arg_name}_awaddr ),
    .m_axi_{arg_name}_AWBURST (axi_{arg_name}_awburst),
    .m_axi_{arg_name}_AWCACHE (axi_{arg_name}_awcache),
    .m_axi_{arg_name}_AWID    (axi_{arg_name}_awid   ),
    .m_axi_{arg_name}_AWLEN   (axi_{arg_name}_awlen  ),
    .m_axi_{arg_name}_AWLOCK  (axi_{arg_name}_awlock ),
    .m_axi_{arg_name}_AWPROT  (axi_{arg_name}_awprot ),
    .m_axi_{arg_name}_AWQOS   (axi_{arg_name}_awqos  ),
    .m_axi_{arg_name}_AWREADY (axi_{arg_name}_awready),
    .m_axi_{arg_name}_AWSIZE  (axi_{arg_name}_awsize ),
    .m_axi_{arg_name}_AWVALID (axi_{arg_name}_awvalid),
    .m_axi_{arg_name}_BID     (axi_{arg_name}_bid    ),
    .m_axi_{arg_name}_BREADY  (axi_{arg_name}_bready ),
    .m_axi_{arg_name}_BRESP   (axi_{arg_name}_bresp  ),
    .m_axi_{arg_name}_BVALID  (axi_{arg_name}_bvalid ),
    .m_axi_{arg_name}_RDATA   (axi_{arg_name}_rdata  ),
    .m_axi_{arg_name}_RID     (axi_{arg_name}_rid    ),
    .m_axi_{arg_name}_RLAST   (axi_{arg_name}_rlast  ),
    .m_axi_{arg_name}_RREADY  (axi_{arg_name}_rready ),
    .m_axi_{arg_name}_RRESP   (axi_{arg_name}_rresp  ),
    .m_axi_{arg_name}_RVALID  (axi_{arg_name}_rvalid ),
    .m_axi_{arg_name}_WDATA   (axi_{arg_name}_wdata  ),
    .m_axi_{arg_name}_WLAST   (axi_{arg_name}_wlast  ),
    .m_axi_{arg_name}_WREADY  (axi_{arg_name}_wready ),
    .m_axi_{arg_name}_WSTRB   (axi_{arg_name}_wstrb  ),
    .m_axi_{arg_name}_WVALID  (axi_{arg_name}_wvalid ),
"""


def get_vitis_dut(top_name: str, args: Sequence[Arg]) -> str:
    dut = f"""
  {top_name} dut (
    .s_axi_control_AWVALID (s_axi_control_awvalid),
    .s_axi_control_AWREADY (s_axi_control_awready),
    .s_axi_control_AWADDR  (s_axi_control_awaddr ),

    .s_axi_control_WVALID  (s_axi_control_wvalid ),
    .s_axi_control_WREADY  (s_axi_control_wready ),
    .s_axi_control_WDATA   (s_axi_control_wdata  ),
    .s_axi_control_WSTRB   ({{64{{1'b1}} }}      ),

    // keep polling the control registers
    .s_axi_control_ARVALID (s_axi_control_arvalid),
    .s_axi_control_ARREADY (s_axi_control_arready),
    .s_axi_control_ARADDR  ('h00 ),

    .s_axi_control_RVALID  (s_axi_control_rvalid ),
    .s_axi_control_RREADY  (1 ),
    .s_axi_control_RDATA   (s_axi_control_rdata  ),
    .s_axi_control_RRESP   (                     ),

    .s_axi_control_BVALID  (s_axi_control_bvalid ),
    .s_axi_control_BREADY  (1'b1                 ),
    .s_axi_control_BRESP   (s_axi_control_bresp  ),
"""

    for arg in args:
        if arg.is_mmap:
            dut += get_m_axi_connections(arg.name)
        if arg.is_stream:
            dut += f"""
    .{arg.name}_TDATA  (axis_{arg.name}_tdata ),
    .{arg.name}_TVALID (axis_{arg.name}_tvalid),
    .{arg.name}_TREADY (axis_{arg.name}_tready),
    .{arg.name}_TLAST  (axis_{arg.name}_tlast ),
"""

    dut += """
    .ap_clk          (ap_clk       ),
    .ap_rst_n        (ap_rst_n     ),
    .interrupt       (             )
  );
  """

    return dut


def get_hls_dut(
    top_name: str,
    top_is_leaf_task: bool,
    args: Sequence[Arg],
    scalar_to_val: dict[str, str],
) -> str:
    dut = f"\n  {top_name} dut (\n"

    for arg in args:
        if arg.is_mmap:
            dut += get_m_axi_connections(arg.name)
            dut += f"""
    .{arg.name}_offset({scalar_to_val.get(arg.name, 0)}),\n
"""

        if arg.is_stream and arg.port.is_istream:
            dut += f"""
    .{arg.qualified_name}_dout(fifo_{arg.qualified_name}_data),
    .{arg.qualified_name}_empty_n(fifo_{arg.qualified_name}_valid),
    .{arg.qualified_name}_read(fifo_{arg.qualified_name}_ready),
"""
            if top_is_leaf_task:
                dut += f"""
    .{arg.peek_qualified_name}_dout(fifo_{arg.qualified_name}_data),
    .{arg.peek_qualified_name}_empty_n(fifo_{arg.qualified_name}_valid),
"""

        if arg.is_stream and arg.port.is_ostream:
            dut += f"""
    .{arg.qualified_name}_din(fifo_{arg.qualified_name}_data),
    .{arg.qualified_name}_full_n(fifo_{arg.qualified_name}_ready),
    .{arg.qualified_name}_write(fifo_{arg.qualified_name}_valid),
"""

        if arg.is_scalar:
            dut += f"""
    .{arg.name}({scalar_to_val.get(arg.name, 0)}),\n
"""

    dut += """
    .ap_clk(ap_clk),
    .ap_rst_n(ap_rst_n),
    .ap_start(ap_start),
    .ap_done(ap_done),
    .ap_ready(ap_ready),
    .ap_idle(ap_idle)
  );
"""

    return dut


def get_vitis_test_signals(
    arg_to_reg_addrs: dict[str, list[str]],
    scalar_arg_to_val: dict[str, str],
    args: Sequence[Arg],
) -> str:
    return render_vitis_test_signals(arg_to_reg_addrs, scalar_arg_to_val, args)


def get_hls_test_signals(args: Sequence[Arg]) -> str:
    return render_hls_test_signals(args)


def get_begin() -> str:
    return render_testbench_begin()


def get_end() -> str:
    return render_testbench_end()


def get_axi_ram_module(axi: AXI, input_data_path: str, c_array_size: int) -> str:
    """Generate the AXI RAM module for cosimulation."""
    return render_axi_ram_module(axi, input_data_path, c_array_size)


def get_srl_fifo_template() -> str:
    return """`default_nettype none

// first-word fall-through (FWFT) FIFO using shift register LUT
// based on HLS generated code
module fifo_srl #(
  parameter MEM_STYLE  = "shiftreg",
  parameter DATA_WIDTH = 512,
  parameter ADDR_WIDTH = 64,
  parameter DEPTH      = 32
) (
  input wire clk,
  input wire reset,

  // write
  output wire                  if_full_n,
  input  wire                  if_write_ce,
  input  wire                  if_write,
  input  wire [DATA_WIDTH-1:0] if_din,

  // read
  output wire                  if_empty_n,
  input  wire                  if_read_ce,
  input  wire                  if_read,
  output wire [DATA_WIDTH-1:0] if_dout
);

  wire [ADDR_WIDTH - 1:0] shift_reg_addr;
  wire [DATA_WIDTH - 1:0] shift_reg_data;
  wire [DATA_WIDTH - 1:0] shift_reg_q;
  wire                    shift_reg_ce;
  reg  [ADDR_WIDTH:0]     out_ptr;
  reg                     internal_empty_n;
  reg                     internal_full_n;

  reg [DATA_WIDTH-1:0] mem [0:DEPTH-1];

  assign if_empty_n = internal_empty_n;
  assign if_full_n = internal_full_n;
  assign shift_reg_data = if_din;
  assign if_dout = shift_reg_q;

  assign shift_reg_addr = out_ptr[ADDR_WIDTH] == 1'b0 ? out_ptr[ADDR_WIDTH-1:0]
                                                      : {ADDR_WIDTH{1'b0}};
  assign shift_reg_ce = (if_write & if_write_ce) & internal_full_n;

  assign shift_reg_q = mem[shift_reg_addr];

  always @(posedge clk) begin
    if (reset) begin
      out_ptr <= ~{ADDR_WIDTH+1{1'b0}};
      internal_empty_n <= 1'b0;
      internal_full_n <= 1'b1;
    end else begin
      if (((if_read && if_read_ce) && internal_empty_n) &&
          (!(if_write && if_write_ce) || !internal_full_n)) begin
        out_ptr <= out_ptr - 1'b1;
        if (out_ptr == {(ADDR_WIDTH+1){1'b0}})
          internal_empty_n <= 1'b0;
        internal_full_n <= 1'b1;
      end
      else if (((if_read & if_read_ce) == 0 | internal_empty_n == 0) &&
        ((if_write & if_write_ce) == 1 & internal_full_n == 1))
      begin
        out_ptr <= out_ptr + 1'b1;
        internal_empty_n <= 1'b1;
        if (out_ptr == DEPTH - {{(ADDR_WIDTH-1){1'b0}}, 2'd2})
          internal_full_n <= 1'b0;
      end
    end
  end

  integer i;
  always @(posedge clk) begin
    if (shift_reg_ce) begin
      for (i = 0; i < DEPTH - 1; i = i + 1)
        mem[i + 1] <= mem[i];
      mem[0] <= shift_reg_data;
    end
  end

endmodule  // fifo_srl

`default_nettype wire
"""
