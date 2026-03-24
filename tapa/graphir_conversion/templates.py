"""Tapa graphir templates."""

from pathlib import Path

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

FIFO_TEMPLATE = (
    Path(__file__).parent.parent / "assets" / "verilog" / "fifo.v"
).read_text(encoding="utf-8")


RESET_INVERTER_TEMPLATE = """
// Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

module reset_inverter (
  input wire clk,
  input wire rst_n,
  output wire rst
);

  assign rst = ~rst_n;

endmodule
"""
