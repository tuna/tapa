"""Packaging helpers for Xilinx-oriented Verilog outputs."""

from __future__ import annotations

import copy
import importlib
import json
import logging
import os
import shutil
import sys
import tempfile
from typing import IO, TYPE_CHECKING, BinaryIO

from tapa.backend.xilinx import Arg, Cat, PackageXo
from tapa.backend.xilinx import print_kernel_xml as print_kernel_xml_backend
from tapa.protocol import (
    HANDSHAKE_CLK,
    HANDSHAKE_OUTPUT_PORTS,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)
from tapa.util import get_indexed_name, range_or_none
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.const import CLK

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

    from pyverilog.vparser.ast import Node, PortArg

    from tapa.instance import Instance, Port

_logger = logging.getLogger().getChild(__name__)


def generate_handshake_ports(
    instance: Instance,
    rst: Node,
    start: Node | None = None,
) -> Iterator[PortArg]:
    yield make_port_arg(port=HANDSHAKE_CLK, arg=CLK)
    yield make_port_arg(port=HANDSHAKE_RST_N, arg=rst)
    if start is None:
        # Import lazily to avoid hard coupling; callers should migrate to
        # passing start explicitly via InstanceSignals.
        from tapa.codegen.instance_signals import InstanceSignals  # noqa: PLC0415

        start = InstanceSignals(instance).start
    yield make_port_arg(port=HANDSHAKE_START, arg=start)
    for port in HANDSHAKE_OUTPUT_PORTS:
        yield make_port_arg(
            port=port,
            arg="" if instance.is_autorun else wire_name(instance.name, port),
        )


def pack(
    top_name: str,
    rtl_dir: str,
    ports: Iterable[Port],
    part_num: str,
    output_file: str | BinaryIO,
) -> None:
    """Create a .xo file that archives all generated RTL files."""
    port_list = []
    _logger.debug("RTL ports of %s:", top_name)
    for port in ports:
        for i in range_or_none(port.chan_count):
            port_i = copy.copy(port)
            port_i.name = get_indexed_name(port.name, i)
            _logger.debug("  %s", port_i)
            port_list.append(port_i)
    if isinstance(output_file, str):
        xo_file = output_file
    else:
        xo_file = tempfile.mktemp(prefix="tapa_" + top_name + "_", suffix=".xo")

    if os.environ.get("TAPA_USE_RUST_XILINX") == "1":
        _pack_via_rust(
            top_name=top_name,
            rtl_dir=rtl_dir,
            port_list=port_list,
            part_num=part_num,
            xo_file=xo_file,
        )
        if not isinstance(output_file, str):
            with open(xo_file, "rb") as xo_obj:
                shutil.copyfileobj(xo_obj, output_file)
            if os.path.exists(xo_file):
                os.remove(xo_file)
        return
    with tempfile.NamedTemporaryFile(
        mode="w+",
        prefix="tapa_" + top_name + "_",
        suffix="_kernel.xml",
        encoding="utf-8",
    ) as kernel_xml_obj:
        print_kernel_xml(name=top_name, ports=port_list, kernel_xml=kernel_xml_obj)
        kernel_xml_obj.flush()
        with PackageXo(
            xo_file=xo_file,
            top_name=top_name,
            kernel_xml=kernel_xml_obj.name,
            hdl_dir=rtl_dir,
            m_axi_names={
                port.name: {
                    "HAS_BURST": "0",
                    "SUPPORTS_NARROW_BURST": "0",
                }
                for port in port_list
                if port.cat.is_mmap
            },
            part_num=part_num,
        ) as proc:
            stdout, stderr = proc.communicate()
        if proc.returncode == 0 and os.path.exists(xo_file):
            if not isinstance(output_file, str):
                with open(xo_file, "rb") as xo_obj:
                    shutil.copyfileobj(xo_obj, output_file)
        else:
            sys.stdout.write(stdout.decode("utf-8"))
            sys.stderr.write(stderr.decode("utf-8"))
    if not isinstance(output_file, str) and os.path.exists(xo_file):
        os.remove(xo_file)


_CAT_TO_RUST = {
    "is_scalar": "Scalar",
    "is_mmap": "MAxi",
    "is_istream": "IStream",
    "is_ostream": "OStream",
}


def _port_to_rust(port: Port) -> dict:
    for attr, category in _CAT_TO_RUST.items():
        if getattr(port.cat, attr):
            break
    else:
        msg = f"unexpected port.cat: {port.cat}"
        raise ValueError(msg)
    return {
        "name": port.name,
        "category": category,
        "width": port.width,
        "port": "",
        "ctype": port.ctype,
    }


def _pack_via_rust(
    top_name: str,
    rtl_dir: str,
    port_list: list[Port],
    part_num: str,
    xo_file: str,
) -> None:
    """Drive `tapa_core.xilinx.pack_xo` in full-pack mode.

    Under `TAPA_USE_RUST_XILINX=1` the Python path is replaced by the
    PyO3 binding. Binding errors are not swallowed — they surface as
    `ValueError` to fail hard at the Python layer.
    """
    xilinx_mod = importlib.import_module("tapa_core.xilinx")
    rust_ports = [_port_to_rust(p) for p in port_list]
    m_axi_params = [
        (p.name, [("HAS_BURST", "0"), ("SUPPORTS_NARROW_BURST", "0")])
        for p in port_list
        if p.cat.is_mmap
    ]
    inputs = {
        "kernel_out_path": xo_file,
        "hdl_dir": rtl_dir,
        "top_name": top_name,
        "part_num": part_num,
        "clock_period": os.environ.get("TAPA_CLOCK_PERIOD", "3.33"),
        "kernel_xml": {
            "top_name": top_name,
            "clock_period": os.environ.get("TAPA_CLOCK_PERIOD", "3.33"),
            "ports": rust_ports,
        },
        "cpp_kernels": [],
        "m_axi_params": m_axi_params,
        "s_axi_ifaces": ["s_axi_control"],
    }
    _logger.info("pack_xo: dispatching to Rust (TAPA_USE_RUST_XILINX=1)")
    xilinx_mod.pack_xo(json.dumps(inputs))


def print_kernel_xml(name: str, ports: Iterable[Port], kernel_xml: IO[str]) -> None:
    """Generate kernel.xml file for XO packaging."""
    args = []
    for port in ports:
        for attr, cat in (
            ("is_scalar", Cat.SCALAR),
            ("is_mmap", Cat.MMAP),
            ("is_istream", Cat.ISTREAM),
            ("is_ostream", Cat.OSTREAM),
        ):
            if getattr(port.cat, attr):
                break
        else:
            msg = f"unexpected port.cat: {port.cat}"
            raise ValueError(msg)
        args.append(
            Arg(cat=cat, name=port.name, port="", ctype=port.ctype, width=port.width)
        )
    print_kernel_xml_backend(name, args, kernel_xml)
