"""Verilog module wrapper using pyslang for parsing and rewriting."""

import logging
import tempfile
from collections.abc import Collection, Iterable
from pathlib import Path

import jinja2
import pyslang
from pyverilog.vparser.ast import Node

from tapa.common.pyslang_rewriter import PyslangRewriter
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.parameter import Parameter
from tapa.verilog.ast.signal import Reg, Wire
from tapa.verilog.xilinx.module_ops.axi import add_m_axi, generate_m_axi_ports
from tapa.verilog.xilinx.module_ops.edits import (
    add_comment_lines,
    add_instance,
    add_logics,
    add_params,
    add_pipeline,
    add_ports,
    add_rs_pragmas,
    add_signals,
    del_instances,
    del_logics,
    del_params,
    del_port,
    del_signals,
)
from tapa.verilog.xilinx.module_ops.fifo import add_fifo_instance, cleanup
from tapa.verilog.xilinx.module_ops.mmap import (
    _AsyncMmapContext,
    add_async_mmap_instance,
)
from tapa.verilog.xilinx.module_ops.parse import parse_syntax_tree
from tapa.verilog.xilinx.module_ops.ports import (
    find_port,
    generate_istream_ports,
    generate_ostream_ports,
    get_port_of,
)

_logger = logging.getLogger().getChild(__name__)

__all__ = [
    "Module",
    "generate_m_axi_ports",
]


class Module:
    """AST and helpers for a verilog module."""

    _module_decl: pyslang.ModuleDeclarationSyntax

    _params: dict[str, Parameter]
    _param_name_to_decl: dict[str, pyslang.ParameterDeclarationStatementSyntax]
    _param_source_range: pyslang.SourceRange

    _ports: dict[str, IOPort]
    _port_name_to_decl: dict[str, pyslang.PortDeclarationSyntax]
    _port_source_range: pyslang.SourceRange

    _signals: dict[str, Wire | Reg]
    _signal_name_to_decl: dict[
        str,
        pyslang.DataDeclarationSyntax | pyslang.NetDeclarationSyntax,
    ]
    _signal_source_range: pyslang.SourceRange

    _logics: list[pyslang.ContinuousAssignSyntax | pyslang.ProceduralBlockSyntax]
    _logic_source_range: pyslang.SourceRange

    _instances: list[pyslang.HierarchyInstantiationSyntax]
    _instance_source_range: pyslang.SourceRange

    def __init__(
        self,
        files: Collection[Path] = (),
        is_trimming_enabled: bool = False,
        name: str = "",
    ) -> None:
        if not files:
            if not name:
                msg = "`files` and `name` cannot both be empty"
                raise ValueError(msg)
            self._syntax_tree = pyslang.SyntaxTree.fromText(
                f"module {name}(); endmodule",
            )
            self._rewriter = PyslangRewriter(self._syntax_tree)
            self._parse_syntax_tree()
            return
        with tempfile.TemporaryDirectory(prefix="pyverilog-") as output_dir:
            if is_trimming_enabled:
                # trim the body since we only need the interface information
                files = [
                    _gen_trimmed_file(file, idx, output_dir)
                    for idx, file in enumerate(files)
                ]
            self._syntax_tree = pyslang.SyntaxTree.fromFiles([str(x) for x in files])
            self._rewriter = PyslangRewriter(self._syntax_tree)
            self._parse_syntax_tree()

    @property
    def name(self) -> str:
        return self._module_decl.header.name.valueText

    @property
    def ports(self) -> dict[str, IOPort]:
        return self._ports

    class NoMatchingPortError(ValueError):
        """No matching port being found exception."""

    get_port_of = get_port_of
    generate_istream_ports = generate_istream_ports
    generate_ostream_ports = generate_ostream_ports

    @property
    def signals(self) -> dict[str, Wire | Reg]:
        return self._signals

    @property
    def params(self) -> dict[str, Parameter]:
        return self._params

    @property
    def code(self) -> str:
        self._syntax_tree = self._rewriter.commit()
        self._parse_syntax_tree()
        return str(self._syntax_tree.root)

    def get_template_code(self) -> str:
        return jinja2.Template("""
module {{name}}
(
{%- for port in ports %}
  {{ port.name }}{% if not loop.last %},{% endif %}
{%- endfor %}
);
{%- for port in ports %}
  {{ port }}
{%- endfor %}
endmodule

""").render(name=self.name, ports=self.ports.values())

    _parse_syntax_tree = parse_syntax_tree
    add_ports = add_ports
    del_port = del_port
    add_comment_lines = add_comment_lines
    add_signals = add_signals
    add_pipeline = add_pipeline
    del_signals = del_signals
    add_params = add_params
    del_params = del_params
    add_instance = add_instance
    add_logics = add_logics
    del_logics = del_logics
    del_instances = del_instances
    add_rs_pragmas = add_rs_pragmas

    add_fifo_instance = add_fifo_instance

    def add_async_mmap_instance(  # noqa: PLR0913,PLR0917
        self,
        name: str,
        tags: Iterable[str],
        rst: Node,
        data_width: int,
        addr_width: int = 64,
        buffer_size: int | None = None,
        max_wait_time: int = 3,
        max_burst_len: int | None = None,
    ) -> "Module":
        return add_async_mmap_instance(
            _AsyncMmapContext(
                module=self,
                name=name,
                tags=tuple(tags),
                rst=rst,
                data_width=data_width,
                addr_width=addr_width,
                buffer_size=buffer_size,
                max_wait_time=max_wait_time,
                max_burst_len=max_burst_len,
            ),
        )

    find_port = find_port
    add_m_axi = add_m_axi
    cleanup = cleanup


def _gen_trimmed_file(file: Path, idx: int, output_dir: str) -> Path:
    """Trim a Verilog file body, keeping only the interface declarations."""
    lines = []
    with open(file, encoding="utf-8") as fp:
        for line in fp:
            items = line.strip().split()
            if (
                len(items) > 1
                and items[0] in {"reg", "wire"}
                and items[1].startswith("ap_rst")
            ):
                lines.append("endmodule")
                break
            lines.append(line)
    new_file = Path(output_dir) / f"trimmed_{idx}.v"
    with open(new_file, "w", encoding="utf-8") as fp:
        fp.writelines(lines)
    return new_file
