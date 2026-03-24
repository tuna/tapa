"""Xilinx IP detection and behavioral replacement generation for Verilator."""

from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

_logger = logging.getLogger().getChild(__name__)
_DOUBLE_WIDTH = 64

if TYPE_CHECKING:
    from pathlib import Path


def detect_xilinx_ips(rtl_dir: Path) -> list[str]:
    replacements = []

    for v_file in sorted(rtl_dir.glob("*.v")):
        content = v_file.read_text(encoding="utf-8", errors="replace")
        ip_insts = re.findall(r"^(\w+_ip)\s+\w+\s*\(", content, re.MULTILINE)
        for ip_module in ip_insts:
            ip_path = rtl_dir / f"{ip_module}.v"
            if ip_path.exists() and "`pragma protect" not in ip_path.read_text(
                encoding="utf-8", errors="replace"
            ):
                continue

            tcl_path = rtl_dir / f"{ip_module}.tcl"
            ip_config = parse_ip_tcl(tcl_path) if tcl_path.exists() else None

            if ip_config is not None:
                dpi_func = ip_config["dpi_func"]
                latency = ip_config["latency"]
            else:
                dpi_func = detect_fp_operation_from_name(ip_module)
                latency = 5
                if dpi_func is None:
                    _logger.warning(
                        "Cannot determine operation for %s -- skipping", ip_module
                    )
                    continue

            replacement = generate_fp_ip_replacement(ip_module, dpi_func, latency)
            ip_path.write_text(replacement, encoding="utf-8")
            replacements.append(ip_module)
            _logger.info(
                "Generated behavioral replacement: %s (using %s, latency=%d)",
                ip_module,
                dpi_func,
                latency,
            )

    return replacements


def parse_ip_tcl(tcl_path: Path) -> dict | None:
    content = tcl_path.read_text(encoding="utf-8", errors="replace")
    if "create_ip -name floating_point" not in content:
        return None

    config = {
        m.group(1): m.group(2).rstrip("\\")
        for m in re.finditer(r"CONFIG\.(\w+)\s+(\S+)", content)
    }

    is_double = config.get("a_precision_type", "Single").lower() == "double"
    op_type = config.get("operation_type", "")
    if "Add" in op_type or "Subtract" in op_type:
        add_sub = config.get("add_sub_value", "Add")
        op = "sub" if add_sub.lower() in {"subtract", "sub"} else "add"
    elif "Multiply" in op_type:
        op = "mul"
    else:
        return None

    dpi_func = f"{'fp64' if is_double else 'fp32'}_{op}"
    return {"dpi_func": dpi_func, "latency": int(config.get("c_latency", "5"))}


_FP_NAME_MAP = {
    "fadd": "fp32_add",
    "fsub": "fp32_sub",
    "fmul": "fp32_mul",
    "dadd": "fp64_add",
    "dsub": "fp64_sub",
    "dmul": "fp64_mul",
}


def detect_fp_operation_from_name(module_name: str) -> str | None:
    lower = module_name.lower()
    for pattern, func in _FP_NAME_MAP.items():
        if f"_{pattern}_" in lower or f"_{pattern}s_" in lower:
            return func
    return None


def generate_fp_ip_replacement(
    module_name: str, dpi_func: str, latency: int = 5
) -> str:
    bit_width = _DOUBLE_WIDTH if "64" in dpi_func else 32
    ret_type = "longint unsigned" if bit_width == _DOUBLE_WIDTH else "int unsigned"
    arg_type = ret_type

    return f"""\n\
`timescale 1ns/1ps

module {module_name} (
    input  wire        aclk,
    input  wire        aclken,
    input  wire        s_axis_a_tvalid,
    input  wire [{bit_width - 1}:0] s_axis_a_tdata,
    input  wire        s_axis_b_tvalid,
    input  wire [{bit_width - 1}:0] s_axis_b_tdata,
    output wire        m_axis_result_tvalid,
    output wire [{bit_width - 1}:0] m_axis_result_tdata
);

import "DPI-C" function {ret_type} {dpi_func}(
input {arg_type} a, input {arg_type} b);

reg [{bit_width - 1}:0] pipe [0:{latency - 1}];
reg [{latency - 1}:0]  valid_pipe;

integer i;

always @(posedge aclk) begin
    if (aclken) begin
        pipe[0] <= {dpi_func}(s_axis_a_tdata, s_axis_b_tdata);
        valid_pipe[0] <= s_axis_a_tvalid & s_axis_b_tvalid;
        for (i = 1; i < {latency}; i = i + 1) begin
            pipe[i] <= pipe[i-1];
            valid_pipe[i] <= valid_pipe[i-1];
        end
    end
end

assign m_axis_result_tdata  = pipe[{latency - 1}];
assign m_axis_result_tvalid = valid_pipe[{latency - 1}];

endmodule
"""
