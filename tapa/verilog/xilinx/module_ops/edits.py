# ruff: noqa: SLF001
"""Mutation helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

from typing import TYPE_CHECKING

import pyslang
from pyverilog.ast_code_generator.codegen import ASTCodeGenerator
from pyverilog.vparser.ast import Instance, InstanceList, ParamArg, PortArg

from tapa.verilog.ast.logic import Assign
from tapa.verilog.xilinx.module_ops.axi import _get_rs_pragma

if TYPE_CHECKING:
    from collections.abc import Iterable

    from pyverilog.vparser.ast import Node

    from tapa.verilog.ast.ioport import IOPort
    from tapa.verilog.ast.logic import Always
    from tapa.verilog.ast.parameter import Parameter
    from tapa.verilog.ast.signal import Reg, Wire
    from tapa.verilog.util import Pipeline
    from tapa.verilog.xilinx.module import Module

_CODEGEN = ASTCodeGenerator()


def add_ports(module: Module, ports: Iterable[IOPort]) -> Module:
    """Add IO ports to a module."""
    header_pieces = []
    body_pieces = []
    is_ports_empty = len(module._ports) == 0
    for port in ports:
        module._ports[port.name] = port
        header_pieces.extend([",\n  ", port.name])
        body_pieces.extend(["\n  ", str(port)])
    if is_ports_empty and header_pieces:
        header_pieces[0] = "  "

    module._rewriter.add_before(
        module._module_decl.header.ports.getLastToken().location,
        header_pieces,
    )
    module._rewriter.add_before(module._port_source_range.end, body_pieces)
    return module


def del_port(module: Module, port_name: str) -> None:
    if module._ports.pop(port_name, None) is None:
        msg = f"no port {port_name} found in module {module.name}"
        raise ValueError(msg)

    module._rewriter.remove(module._port_name_to_decl[port_name].sourceRange)

    non_ansi_port_list = module._module_decl.header.ports
    assert isinstance(non_ansi_port_list, pyslang.NonAnsiPortListSyntax)

    nodes = []
    tokens = []
    index_to_del = -1
    for i, node_or_token in enumerate(non_ansi_port_list.ports):
        if i % 2 == 0:
            assert isinstance(node_or_token, pyslang.ImplicitNonAnsiPortSyntax)
            assert isinstance(node_or_token.expr, pyslang.PortReferenceSyntax)
            nodes.append(node_or_token)
            if node_or_token.expr.name.valueText == port_name:
                index_to_del = i // 2
        else:
            assert isinstance(node_or_token, pyslang.Token)
            assert node_or_token.valueText == ","
            tokens.append(node_or_token)
    assert len(nodes) == len(tokens) + 1

    if index_to_del == -1:
        msg = f"no port {port_name} found in module {module.name}"
        raise ValueError(msg)

    module._rewriter.remove(nodes[index_to_del].sourceRange)
    if index_to_del == len(nodes) - 1:
        index_to_del = -1
    module._rewriter.remove(tokens[index_to_del].range)


def add_comment_lines(module: Module, lines: Iterable[str]) -> Module:
    pieces = ["\n"]
    for line in lines:
        if not line.startswith("// "):
            msg = f"line must start with `// `, got `{line}`"
            raise ValueError(msg)
        if "\n" in line:
            msg = f"line must not contain newlines`, got `{line}`"
            raise ValueError(msg)
        pieces.append(line)
        pieces.append("\n")

    module._rewriter.add_before(module._module_decl.header.sourceRange.end, pieces)
    return module


def add_signals(module: Module, signals: Iterable[Wire | Reg]) -> Module:
    for signal in signals:
        module._signals[signal.name] = signal
        module._rewriter.add_before(
            module._signal_source_range.end,
            ["\n  ", str(signal)],
        )
    return module


def add_pipeline(module: Module, q: Pipeline, init: Node) -> None:
    module.add_signals(q.signals)
    module.add_logics([Assign(lhs=q[0].name, rhs=_CODEGEN.visit(init))])


def del_signals(module: Module, prefix: str = "", suffix: str = "") -> None:
    new_signals = {}
    for name, signal in module._signals.items():
        if name.startswith(prefix) and name.endswith(suffix):
            module._rewriter.remove(module._signal_name_to_decl[name].sourceRange)
        else:
            new_signals[name] = signal
    module._signals = new_signals


def add_params(module: Module, params: Iterable[Parameter]) -> Module:
    for param in params:
        module._params[param.name] = param
        module._rewriter.add_before(
            module._param_source_range.end,
            ["\n  ", str(param)],
        )
    return module


def del_params(module: Module, prefix: str = "", suffix: str = "") -> None:
    new_params = {}
    for name, param in module._params.items():
        if name.startswith(prefix) and name.endswith(suffix):
            module._rewriter.remove(module._param_name_to_decl[name].sourceRange)
        else:
            new_params[name] = param
    module._params = new_params


def add_instance(
    module: Module,
    module_name: str,
    instance_name: str,
    ports: Iterable[PortArg],
    params: Iterable[ParamArg] = (),
) -> Module:
    item = InstanceList(
        module=module_name,
        parameterlist=tuple(params),
        instances=(
            Instance(
                module=None,
                name=instance_name,
                parameterlist=None,
                portlist=tuple(ports),
            ),
        ),
    )
    module._rewriter.add_before(module._instance_source_range.end, ["\n  ", str(item)])
    return module


def add_logics(module: Module, logics: Iterable[Assign | Always]) -> Module:
    for logic in logics:
        module._rewriter.add_before(
            module._logic_source_range.end,
            ["\n  ", str(logic)],
        )
    return module


def del_logics(module: Module) -> None:
    for logic in module._logics:
        module._rewriter.remove(logic.sourceRange)


def del_instances(module: Module, prefix: str = "", suffix: str = "") -> None:
    for instance in module._instances:
        module_name = instance.type.valueText
        if module_name.startswith(prefix) and module_name.endswith(suffix):
            module._rewriter.remove(instance.sourceRange)


def add_rs_pragmas(module: Module) -> Module:
    module._syntax_tree = module._rewriter.commit()
    module._parse_syntax_tree()
    for port in module._ports.values():
        if (pragma := _get_rs_pragma(port.name)) is not None:
            module._rewriter.add_before(
                module._port_name_to_decl[port.name].sourceRange.start,
                str(pragma),
            )
    return module
