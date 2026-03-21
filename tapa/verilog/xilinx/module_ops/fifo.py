"""FIFO-related helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pyverilog.vparser.ast import Constant, Node, ParamArg, PortArg

from tapa.verilog.ast.logic import Assign
from tapa.verilog.ast.signal import Wire
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.util import sanitize_array_name, wire_name
from tapa.verilog.xilinx.const import (
    CLK,
    FIFO_READ_PORTS,
    FIFO_WRITE_PORTS,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_RST,
    HANDSHAKE_RST_N,
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    TRUE,
)

if TYPE_CHECKING:
    from collections.abc import Iterator

    from tapa.verilog.xilinx.module import Module


def add_fifo_instance(
    module: Module,
    name: str,
    rst: Node,
    width: Node,
    depth: int,
) -> Module:
    name = sanitize_array_name(name)

    def ports() -> Iterator[PortArg]:
        yield make_port_arg(port="clk", arg=CLK)
        yield make_port_arg(port="reset", arg=rst)
        for port_name, arg_suffix in zip(FIFO_READ_PORTS, ISTREAM_SUFFIXES):
            yield make_port_arg(port=port_name, arg=wire_name(name, arg_suffix))

        yield make_port_arg(port=FIFO_READ_PORTS[-1], arg=TRUE)
        for port_name, arg_suffix in zip(FIFO_WRITE_PORTS, OSTREAM_SUFFIXES):
            yield make_port_arg(port=port_name, arg=wire_name(name, arg_suffix))
        yield make_port_arg(port=FIFO_WRITE_PORTS[-1], arg=TRUE)

    module_name = "fifo"
    return module.add_instance(
        module_name=module_name,
        instance_name=name,
        ports=ports(),
        params=(
            ParamArg(paramname="DATA_WIDTH", argname=width),
            ParamArg(
                paramname="ADDR_WIDTH",
                argname=Constant(max(1, (depth - 1).bit_length())),
            ),
            ParamArg(paramname="DEPTH", argname=Constant(depth)),
        ),
    )


def cleanup(module: Module) -> None:
    module.del_params(prefix="ap_ST_fsm_state")
    module.del_signals(prefix="ap_CS_fsm")
    module.del_signals(prefix="ap_NS_fsm")
    module.del_signals(suffix="_read")
    module.del_signals(suffix="_write")
    module.del_signals(suffix="_blk_n")
    module.del_signals(suffix="_regslice")
    module.del_signals(prefix="regslice_")
    module.del_signals(HANDSHAKE_RST)
    module.del_signals(HANDSHAKE_DONE)
    module.del_signals(HANDSHAKE_IDLE)
    module.del_signals(HANDSHAKE_READY)
    module.del_logics()
    module.del_instances(suffix="_regslice_both")
    module.add_signals(
        map(
            Wire,
            (
                HANDSHAKE_RST,
                HANDSHAKE_DONE,
                HANDSHAKE_IDLE,
                HANDSHAKE_READY,
            ),
        ),
    )
    module.add_logics(
        [
            # `s_axi_control` still uses `ap_rst_n_inv`.
            Assign(lhs=HANDSHAKE_RST, rhs=f"~{HANDSHAKE_RST_N}"),
        ],
    )
