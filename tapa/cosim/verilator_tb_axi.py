"""AXI service and control-port code generation for Verilator TBs."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg


def generate_axi_helpers(axi_list: list[AXI], mode: str) -> list[str]:
    lines: list[str] = []

    lines.append("static void service_all_axi() {")
    for axi in axi_list:
        data_bytes = axi.data_width // 8
        lines.extend(_generate_axi_read_service(axi.name, data_bytes))
        lines.extend(_generate_axi_write_service(axi.name, data_bytes))
    lines.extend(["}", ""])

    if mode == "vitis":
        lines.extend(_CTRL_WRITE_FUNC)

    return lines


def generate_ctrl_writes(
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
) -> list[str]:
    lines: list[str] = [
        "    ctrl_write(0x04, 1);  // GIE",
        "    ctrl_write(0x08, 1);  // IER",
        "",
    ]
    for arg in args:
        mmap_reg_name = arg.name
        if arg.is_mmap and mmap_reg_name not in reg_addrs:
            mmap_reg_name = f"{arg.name}_offset"
        if arg.is_mmap and mmap_reg_name in reg_addrs:
            addrs = reg_addrs[mmap_reg_name]
            base = f"{arg.name}_BASE"
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                val = f"(uint32_t){base}" if i == 0 else f"(uint32_t)({base} >> 32)"
                lines.append(f"    ctrl_write({a}, {val});")

    scalar_to_val = config.get("scalar_to_val", {})
    for arg in args:
        if arg.is_scalar and arg.qualified_name in reg_addrs:
            addrs = reg_addrs[arg.qualified_name]
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                v = (
                    f"(uint32_t){hex_val}"
                    if i == 0
                    else f"(uint32_t)((uint64_t){hex_val} >> 32)"
                )
                lines.append(f"    ctrl_write({a}, {v});")

    lines.extend(["", "    ctrl_write(0x00, 1);"])
    return lines


def generate_hls_port_setup(
    args: Sequence[Arg],
    axi_list: list[AXI],
    config: dict,
) -> list[str]:
    lines: list[str] = []
    scalar_to_val = config.get("scalar_to_val", {})

    for arg in args:
        if arg.is_mmap:
            for idx, axi in enumerate(axi_list):
                if axi.name == arg.name:
                    base = f"0x{idx + 1}0000000ULL"
                    lines.append(f"    dut->{arg.name}_offset = {base};")
                    break

    for arg in args:
        if arg.is_scalar:
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            lines.append(f"    dut->{arg.name} = {hex_val};")

    lines.extend(
        [
            "",
            "    dut->ap_start = 1;",
            "    service_all_axi();",
            "    tick();",
        ]
    )
    return lines


def _generate_axi_read_service(name: str, data_bytes: int) -> list[str]:
    return [
        "    // AXI read service for " + name,
        f"    dut->m_axi_{name}_ARREADY = !rd_{name}.busy;",
        f"    if (dut->m_axi_{name}_ARVALID && !rd_{name}.busy) {{",
        f"        rd_{name}.busy = true;",
        f"        rd_{name}.addr = dut->m_axi_{name}_ARADDR;",
        f"        rd_{name}.len = dut->m_axi_{name}_ARLEN;",
        f"        rd_{name}.id = dut->m_axi_{name}_ARID;",
        f"        rd_{name}.beat = 0;",
        "    }",
        f"    if (rd_{name}.busy) {{",
        f"        dut->m_axi_{name}_RVALID = 1;",
        f"        mem_read(rd_{name}.addr + (uint64_t)rd_{name}.beat * {data_bytes},",
        f"                 &dut->m_axi_{name}_RDATA, {data_bytes});",
        f"        dut->m_axi_{name}_RLAST = (rd_{name}.beat == rd_{name}.len) ? 1 : 0;",
        f"        dut->m_axi_{name}_RID = rd_{name}.id;",
        f"        dut->m_axi_{name}_RRESP = 0;",
        f"        if (dut->m_axi_{name}_RREADY) {{",
        f"            if (rd_{name}.beat >= rd_{name}.len) rd_{name}.busy = false;",
        f"            else rd_{name}.beat++;",
        "        }",
        "    } else {",
        f"        dut->m_axi_{name}_RVALID = 0;",
        "    }",
    ]


def _generate_axi_write_service(name: str, data_bytes: int) -> list[str]:
    return [
        "    // AXI write service for " + name,
        (
            f"    dut->m_axi_{name}_AWREADY ="
            f" (!wr_{name}.aw_got && !wr_{name}.b_pending) ? 1 : 0;"
        ),
        f"    if (dut->m_axi_{name}_AWVALID && dut->m_axi_{name}_AWREADY) {{",
        f"        wr_{name}.aw_got = true;",
        f"        wr_{name}.addr = dut->m_axi_{name}_AWADDR;",
        f"        wr_{name}.id = dut->m_axi_{name}_AWID;",
        f"        wr_{name}.beat = 0;",
        "    }",
        f"    dut->m_axi_{name}_WREADY = wr_{name}.aw_got ? 1 : 0;",
        f"    if (dut->m_axi_{name}_WVALID && dut->m_axi_{name}_WREADY) {{",
        f"        mem_write(wr_{name}.addr + (uint64_t)wr_{name}.beat * {data_bytes},",
        f"                  &dut->m_axi_{name}_WDATA, dut->m_axi_{name}_WSTRB,",
        f"                  {data_bytes});",
        f"        wr_{name}.beat++;",
        f"        if (dut->m_axi_{name}_WLAST) {{",
        f"            wr_{name}.aw_got = false;",
        f"            wr_{name}.b_pending = true;",
        "        }",
        "    }",
        f"    dut->m_axi_{name}_BVALID = wr_{name}.b_pending ? 1 : 0;",
        f"    dut->m_axi_{name}_BID = wr_{name}.id;",
        f"    dut->m_axi_{name}_BRESP = 0;",
        f"    if (wr_{name}.b_pending && dut->m_axi_{name}_BREADY)",
        f"        wr_{name}.b_pending = false;",
    ]


_CTRL_WRITE_FUNC = [
    "static void ctrl_write(uint8_t addr, uint32_t data) {",
    "    dut->s_axi_control_AWVALID = 1;",
    "    dut->s_axi_control_AWADDR = addr;",
    "    dut->s_axi_control_WVALID = 1;",
    "    dut->s_axi_control_WDATA = data;",
    "    dut->s_axi_control_WSTRB = 0xF;",
    "    dut->s_axi_control_BREADY = 1;",
    "    for (int i = 0; i < 20; i++) {",
    "        service_all_axi(); tick();",
    "        if (dut->s_axi_control_BVALID) break;",
    "    }",
    "    dut->s_axi_control_AWVALID = 0;",
    "    dut->s_axi_control_WVALID = 0;",
    "    dut->s_axi_control_BREADY = 0;",
    "    service_all_axi(); tick();",
    "}",
    "",
]
