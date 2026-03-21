__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import functools
import logging
import tempfile
from collections.abc import Collection, Iterable, Iterator
from pathlib import Path

import jinja2
import pyslang
from pyverilog.ast_code_generator.codegen import ASTCodeGenerator
from pyverilog.vparser.ast import Node, PortArg

from tapa.common.pyslang_rewriter import PyslangRewriter
from tapa.common.unique_attrs import UniqueAttrs
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.parameter import Parameter
from tapa.verilog.ast.signal import Reg, Wire
from tapa.verilog.ast.width import Width
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

_CODEGEN = ASTCodeGenerator()
_SIGNAL_SYNTAX = pyslang.DataDeclarationSyntax | pyslang.NetDeclarationSyntax
_LOGIC_SYNTAX = pyslang.ContinuousAssignSyntax | pyslang.ProceduralBlockSyntax


def _get_name(
    node: pyslang.DataDeclarationSyntax | pyslang.NetDeclarationSyntax,
) -> str:
    return node.declarators[0].name.valueText


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
    _signal_name_to_decl: dict[str, _SIGNAL_SYNTAX]
    _signal_source_range: pyslang.SourceRange

    _logics: list[_LOGIC_SYNTAX]
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

    def _parse_syntax_tree(self) -> None:
        """Parse syntax tree and memorize relevant nodes.

        All private attributes (except `_syntax_tree` and `_rewriter`) will be
        created/updated.
        """

        class Attrs(UniqueAttrs):
            module_decl: pyslang.ModuleDeclarationSyntax

        attrs = Attrs()

        self._params = {}
        self._param_name_to_decl = {}
        self._ports = {}
        self._port_name_to_decl = {}
        self._signals = {}
        self._signal_name_to_decl = {}
        self._logics = []
        self._instances = []

        @functools.singledispatch
        def visitor(_: object) -> pyslang.VisitAction:
            return pyslang.VisitAction.Advance

        @visitor.register
        def _(node: pyslang.ModuleDeclarationSyntax) -> pyslang.VisitAction:
            attrs.module_decl = node
            # Append after the header by default.
            self._update_source_range_for_param(node.header)
            return pyslang.VisitAction.Advance

        @visitor.register
        def _(node: pyslang.ParameterDeclarationStatementSyntax) -> pyslang.VisitAction:
            param = Parameter.create(node)
            self._params[param.name] = param
            self._param_name_to_decl[param.name] = node
            self._update_source_range_for_param(node)
            return pyslang.VisitAction.Skip

        @visitor.register
        def _(node: pyslang.PortDeclarationSyntax) -> pyslang.VisitAction:
            port = IOPort.create(node)
            self._ports[port.name] = port
            self._port_name_to_decl[port.name] = node
            self._update_source_range_for_port(node)
            return pyslang.VisitAction.Skip

        @visitor.register
        def _(node: _SIGNAL_SYNTAX) -> pyslang.VisitAction:
            signal = {
                pyslang.DataDeclarationSyntax: Reg,
                pyslang.NetDeclarationSyntax: Wire,
            }[type(node)](_get_name(node), Width.create(node.type))
            self._signals[signal.name] = signal
            self._signal_name_to_decl[signal.name] = node
            self._update_source_range_for_signal(node)
            return pyslang.VisitAction.Skip

        @visitor.register
        def _(node: _LOGIC_SYNTAX) -> pyslang.VisitAction:
            self._logics.append(node)
            self._update_source_range_for_logic(node)
            return pyslang.VisitAction.Skip

        @visitor.register
        def _(node: pyslang.HierarchyInstantiationSyntax) -> pyslang.VisitAction:
            self._instances.append(node)
            self._update_source_range_for_instance(node)
            return pyslang.VisitAction.Skip

        self._syntax_tree.root.visit(visitor)

        self._module_decl = attrs.module_decl

    def _update_source_range_for_param(self, node: pyslang.SyntaxNode) -> None:
        self._param_source_range = node.sourceRange
        self._update_source_range_for_port(node)

    def _update_source_range_for_port(self, node: pyslang.SyntaxNode) -> None:
        self._port_source_range = node.sourceRange
        self._update_source_range_for_signal(node)

    def _update_source_range_for_signal(self, node: pyslang.SyntaxNode) -> None:
        self._signal_source_range = node.sourceRange
        self._update_source_range_for_logic(node)

    def _update_source_range_for_logic(self, node: pyslang.SyntaxNode) -> None:
        self._logic_source_range = node.sourceRange
        self._update_source_range_for_instance(node)

    def _update_source_range_for_instance(self, node: pyslang.SyntaxNode) -> None:
        self._instance_source_range = node.sourceRange

    @property
    def name(self) -> str:
        return self._module_decl.header.name.valueText

    @property
    def ports(self) -> dict[str, IOPort]:
        return self._ports

    class NoMatchingPortError(ValueError):
        """No matching port being found exception."""

    def get_port_of(self, fifo: str, suffix: str) -> IOPort:
        """Return the IOPort of the given fifo with the given suffix.

        Args:
          fifo (str): Name of the fifo.
          suffix (str): One of the suffixes in ISTREAM_SUFFIXES or OSTREAM_SUFFIXES.

        Returns:
          IOPort.

        Raises:
          ValueError: Module does not have the port.
        """
        return get_port_of_helper(self, fifo, suffix, self.NoMatchingPortError)

    def generate_istream_ports(
        self,
        port: str,
        arg: str,
        ignore_peek_fifos: Iterable[str] = (),
    ) -> Iterator[PortArg]:
        yield from generate_istream_ports_helper(self, port, arg, ignore_peek_fifos)

    def generate_ostream_ports(
        self,
        port: str,
        arg: str,
    ) -> Iterator[PortArg]:
        yield from generate_ostream_ports_helper(self, port, arg)

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

    def add_fifo_instance(
        self,
        name: str,
        rst: Node,
        width: Node,
        depth: int,
    ) -> "Module":
        return add_fifo_instance_helper(self, name, rst, width, depth)

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

    def find_port(self, prefix: str, suffix: str) -> str | None:
        """Find an IO port with given prefix and suffix in this module."""
        return find_port_helper(self, prefix, suffix)

    def add_m_axi(
        self,
        name: str,
        data_width: int,
        addr_width: int = 64,
        id_width: int | None = None,
    ) -> "Module":
        return add_m_axi_helper(
            self,
            name,
            data_width,
            addr_width=addr_width,
            id_width=id_width,
        )

    def cleanup(self) -> None:
        cleanup_helper(self)
