"""FSM pragma helpers for tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.const import (
    HANDSHAKE_CLK,
    HANDSHAKE_OUTPUT_PORTS,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)

if TYPE_CHECKING:
    from tapa.task import Task


def add_rs_pragmas_to_fsm(task: Task) -> None:
    """Add RS pragmas to the FSM module."""
    port_map_str = " ".join(
        f"{x}={x}" for x in (HANDSHAKE_START, *HANDSHAKE_OUTPUT_PORTS)
    )
    scalar_regex_str = "|".join(
        name
        for x in task.ports.values()
        if not x.cat.is_stream and not x.is_streams  # TODO: refactor port.cat
        # Use _offset suffix for mmap ports, raw name for scalars; skip if unused.
        for name in [f"{x.name}_offset" if not x.cat.is_scalar else x.name]
        if name in task.fsm_module.ports
    )
    scalar_pragma = f" scalar=({scalar_regex_str})" if scalar_regex_str else ""
    pragma_list = [
        f"clk port={HANDSHAKE_CLK}",
        f"rst port={HANDSHAKE_RST_N} active=low",
        f"ap-ctrl {port_map_str}{scalar_pragma}",
    ]
    for instance in task.instances:
        port_list = [HANDSHAKE_START]
        if not instance.is_autorun:
            port_list.extend(HANDSHAKE_OUTPUT_PORTS)
        port_map_str = " ".join(f"{x}={wire_name(instance.name, x)}" for x in port_list)
        if all(arg.cat.is_stream or "'d" in arg.name for arg in instance.args):
            scalar_pragma = ""
        else:
            scalar_pragma = f" scalar={instance.get_instance_arg('.*')}"
        pragma_list.append(f"ap-ctrl {port_map_str}{scalar_pragma}")
    task.fsm_module.add_comment_lines(
        f"// pragma RS {pragma}" for pragma in pragma_list
    )
