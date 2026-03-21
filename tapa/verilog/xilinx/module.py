__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

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
from tapa.verilog.xilinx.module_ops.axi import add_m_axi as add_m_axi_helper
from tapa.verilog.xilinx.module_ops.axi import (
    generate_m_axi_ports as generate_m_axi_ports_helper,
)
from tapa.verilog.xilinx.module_ops.edits import (
    add_comment_lines as add_comment_lines_helper,
)
from tapa.verilog.xilinx.module_ops.edits import add_instance as add_instance_helper
from tapa.verilog.xilinx.module_ops.edits import add_logics as add_logics_helper
from tapa.verilog.xilinx.module_ops.edits import add_params as add_params_helper
from tapa.verilog.xilinx.module_ops.edits import add_pipeline as add_pipeline_helper
from tapa.verilog.xilinx.module_ops.edits import add_ports as add_ports_helper
from tapa.verilog.xilinx.module_ops.edits import add_rs_pragmas as add_rs_pragmas_helper
from tapa.verilog.xilinx.module_ops.edits import add_signals as add_signals_helper
from tapa.verilog.xilinx.module_ops.edits import del_instances as del_instances_helper
from tapa.verilog.xilinx.module_ops.edits import del_logics as del_logics_helper
from tapa.verilog.xilinx.module_ops.edits import del_params as del_params_helper
from tapa.verilog.xilinx.module_ops.edits import del_port as del_port_helper
from tapa.verilog.xilinx.module_ops.edits import del_signals as del_signals_helper
from tapa.verilog.xilinx.module_ops.fifo import (
    add_fifo_instance as add_fifo_instance_helper,
)
from tapa.verilog.xilinx.module_ops.fifo import cleanup as cleanup_helper
from tapa.verilog.xilinx.module_ops.mmap import (
    _AsyncMmapContext,
)
from tapa.verilog.xilinx.module_ops.mmap import (
    add_async_mmap_instance as add_async_mmap_instance_helper,
)
from tapa.verilog.xilinx.module_ops.parse import (
    parse_syntax_tree as parse_syntax_tree_helper,
)
from tapa.verilog.xilinx.module_ops.ports import (
    find_port as find_port_helper,
)
from tapa.verilog.xilinx.module_ops.ports import (
    generate_istream_ports as generate_istream_ports_helper,
)
from tapa.verilog.xilinx.module_ops.ports import (
    generate_ostream_ports as generate_ostream_ports_helper,
)
from tapa.verilog.xilinx.module_ops.ports import (
    get_port_of as get_port_of_helper,
)
from tapa.verilog.xilinx.module_ops.ports import (
    get_streams_fifos as get_streams_fifos_helper,
)

generate_m_axi_ports = generate_m_axi_ports_helper
get_streams_fifos = get_streams_fifos_helper

_logger = logging.getLogger().getChild(__name__)

__all__ = [
    "Module",
    "generate_m_axi_ports",
]

# vitis hls generated port infixes
FIFO_INFIXES = ("_V", "_r", "_s", "")


class Module:  # TODO: refactor this class
    """AST and helpers for a verilog module.

    Attributes:
        _syntax_tree: Syntax tree parsed from the source.
        _rewriter: Rewriter holding all uncommitted changes to the source.

        _module_decl: Singleton syntax node of the module declaration.

        _params: A dict mapping parameter names to `Parameter` AST nodes.
            Changes to the parameters are always reflected.
        _param_name_to_decl: A dict mapping parameter names to syntax nodes.
            Changes to the parameters are not reflected until they are committed.
        _param_source_range: Syntax node to which the next parameter should be
            appended.

        _ports: A dict mapping port names to `IOPort` AST nodes. Changes to the
            ports are always reflected.
        _port_name_to_decl: A dict mapping port names to syntax nodes. Changes
            to the ports are not reflected until they are committed.
        _port_source_range: Syntax node to which the next port should be
            appended.

        _signals: A dict mapping signal names to `Wire`/`Reg` AST nodes. Changes
            to the signals are always reflected.
        _signal_name_to_decl: A dict mapping signal names to syntax nodes.
            Changes to the signals are not reflected until they are committed.
        _signal_source_range: Syntax node to which the next signal should be
            appended.

        _logics: A list of logic syntax nodes. Changes to the logics are not
            reflected until they are committed.
        _logic_source_range: Syntax node to which the next logic should be
            appended.

        _instances: A list of instance syntax nodes. Changes to the instances
            are not reflected until they are committed.
        _instance_source_range: Syntax node to which the next instance should be
            appended.
    """

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
        """Construct a Module from files."""
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
                new_files = []

                def gen_trimmed_file(file: Path, idx: int) -> Path:
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

                for idx, file in enumerate(files):
                    new_files.append(gen_trimmed_file(file, idx))
                files = new_files
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

    get_port_of = get_port_of_helper
    generate_istream_ports = generate_istream_ports_helper
    generate_ostream_ports = generate_ostream_ports_helper

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

    _parse_syntax_tree = parse_syntax_tree_helper
    add_ports = add_ports_helper
    del_port = del_port_helper
    add_comment_lines = add_comment_lines_helper
    add_signals = add_signals_helper
    add_pipeline = add_pipeline_helper
    del_signals = del_signals_helper
    add_params = add_params_helper
    del_params = del_params_helper
    add_instance = add_instance_helper
    add_logics = add_logics_helper
    del_logics = del_logics_helper
    del_instances = del_instances_helper
    add_rs_pragmas = add_rs_pragmas_helper

    add_fifo_instance = add_fifo_instance_helper

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
        return add_async_mmap_instance_helper(
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

    find_port = find_port_helper
    add_m_axi = add_m_axi_helper
    cleanup = cleanup_helper
