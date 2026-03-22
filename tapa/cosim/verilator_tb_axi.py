"""AXI service and control-port code generation for Verilator TBs."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg


def generate_axi_helpers(axi_list: list[AXI], mode: str) -> list[str]:
    """Generate AXI service functions and ctrl_write."""
    lines: list[str] = []

    lines.append("static void service_all_axi() {")
    for axi in axi_list:
        n = axi.name
        data_bytes = axi.data_width // 8
        lines.extend(_generate_axi_read_service(n, data_bytes))
        lines.extend(_generate_axi_write_service(n, data_bytes))
    lines.extend(["}", ""])

    if mode == "vitis":
        lines.extend(_CTRL_WRITE_FUNC)

    return lines


def generate_ctrl_writes(
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
) -> list[str]:
    """Generate control register write statements for Vitis mode."""
    lines: list[str] = [
        "    // Enable interrupt for done detection",
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

    lines.extend(["", "    // Start kernel", "    ctrl_write(0x00, 1);"])
    return lines


def generate_hls_port_setup(
    args: Sequence[Arg],
    axi_list: list[AXI],
    config: dict,
) -> list[str]:
    """Generate direct port assignments for HLS mode."""
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
            "    // Start kernel",
            "    dut->ap_start = 1;",
            "    service_all_axi();",
            "    tick();",
        ]
    )
    return lines


def _generate_axi_read_service(name: str, data_bytes: int) -> list[str]:
    """Generate inline AXI read service code for one port."""
    n = name
    return [
        "    // AXI read service for " + n,
        f"    dut->m_axi_{n}_ARREADY = !rd_{n}.busy;",
        f"    if (dut->m_axi_{n}_ARVALID && !rd_{n}.busy) {{",
        f"        rd_{n}.busy = true;",
        f"        rd_{n}.addr = dut->m_axi_{n}_ARADDR;",
        f"        rd_{n}.len = dut->m_axi_{n}_ARLEN;",
        f"        rd_{n}.id = dut->m_axi_{n}_ARID;",
        f"        rd_{n}.beat = 0;",
        "    }",
        f"    if (rd_{n}.busy) {{",
        f"        dut->m_axi_{n}_RVALID = 1;",
        f"        mem_read(rd_{n}.addr + (uint64_t)rd_{n}.beat * {data_bytes},",
        f"                 &dut->m_axi_{n}_RDATA, {data_bytes});",
        f"        dut->m_axi_{n}_RLAST = (rd_{n}.beat == rd_{n}.len) ? 1 : 0;",
        f"        dut->m_axi_{n}_RID = rd_{n}.id;",
        f"        dut->m_axi_{n}_RRESP = 0;",
        f"        if (dut->m_axi_{n}_RREADY) {{",
        f"            if (rd_{n}.beat >= rd_{n}.len) rd_{n}.busy = false;",
        f"            else rd_{n}.beat++;",
        "        }",
        "    } else {",
        f"        dut->m_axi_{n}_RVALID = 0;",
        "    }",
    ]


def _generate_axi_write_service(name: str, data_bytes: int) -> list[str]:
    """Generate inline AXI write service code for one port."""
    n = name
    return [
        "    // AXI write service for " + n,
        (
            f"    dut->m_axi_{n}_AWREADY ="
            f" (!wr_{n}.aw_got && !wr_{n}.b_pending) ? 1 : 0;"
        ),
        f"    if (dut->m_axi_{n}_AWVALID && dut->m_axi_{n}_AWREADY) {{",
        f"        wr_{n}.aw_got = true;",
        f"        wr_{n}.addr = dut->m_axi_{n}_AWADDR;",
        f"        wr_{n}.id = dut->m_axi_{n}_AWID;",
        f"        wr_{n}.beat = 0;",
        "    }",
        f"    dut->m_axi_{n}_WREADY = wr_{n}.aw_got ? 1 : 0;",
        f"    if (dut->m_axi_{n}_WVALID && dut->m_axi_{n}_WREADY) {{",
        f"        mem_write(wr_{n}.addr + (uint64_t)wr_{n}.beat * {data_bytes},",
        f"                  &dut->m_axi_{n}_WDATA, dut->m_axi_{n}_WSTRB,",
        f"                  {data_bytes});",
        f"        wr_{n}.beat++;",
        f"        if (dut->m_axi_{n}_WLAST) {{",
        f"            wr_{n}.aw_got = false;",
        f"            wr_{n}.b_pending = true;",
        "        }",
        "    }",
        f"    dut->m_axi_{n}_BVALID = wr_{n}.b_pending ? 1 : 0;",
        f"    dut->m_axi_{n}_BID = wr_{n}.id;",
        f"    dut->m_axi_{n}_BRESP = 0;",
        f"    if (wr_{n}.b_pending && dut->m_axi_{n}_BREADY)",
        f"        wr_{n}.b_pending = false;",
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
