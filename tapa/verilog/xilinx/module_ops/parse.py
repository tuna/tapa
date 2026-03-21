# ruff: noqa: SLF001
"""Parsing helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

import functools
from typing import TYPE_CHECKING

import pyslang

from tapa.common.unique_attrs import UniqueAttrs
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.parameter import Parameter
from tapa.verilog.ast.signal import Reg, Wire
from tapa.verilog.ast.width import Width

if TYPE_CHECKING:
    from tapa.verilog.xilinx.module import Module


def _update_source_range_for_param(module: Module, node: pyslang.SyntaxNode) -> None:
    module._param_source_range = node.sourceRange
    _update_source_range_for_port(module, node)


def _update_source_range_for_port(module: Module, node: pyslang.SyntaxNode) -> None:
    module._port_source_range = node.sourceRange
    _update_source_range_for_signal(module, node)


def _update_source_range_for_signal(
    module: Module,
    node: pyslang.SyntaxNode,
) -> None:
    module._signal_source_range = node.sourceRange
    _update_source_range_for_logic(module, node)


def _update_source_range_for_logic(module: Module, node: pyslang.SyntaxNode) -> None:
    module._logic_source_range = node.sourceRange
    _update_source_range_for_instance(module, node)


def _update_source_range_for_instance(
    module: Module,
    node: pyslang.SyntaxNode,
) -> None:
    module._instance_source_range = node.sourceRange


def parse_syntax_tree(module: Module) -> None:
    """Parse syntax tree and memorize relevant nodes."""

    class Attrs(UniqueAttrs):
        module_decl: pyslang.ModuleDeclarationSyntax

    attrs = Attrs()

    module._params = {}
    module._param_name_to_decl = {}
    module._ports = {}
    module._port_name_to_decl = {}
    module._signals = {}
    module._signal_name_to_decl = {}
    module._logics = []
    module._instances = []

    @functools.singledispatch
    def visitor(_: object) -> pyslang.VisitAction:
        return pyslang.VisitAction.Advance

    @visitor.register
    def _(node: pyslang.ModuleDeclarationSyntax) -> pyslang.VisitAction:
        attrs.module_decl = node
        _update_source_range_for_param(module, node.header)
        return pyslang.VisitAction.Advance

    @visitor.register
    def _(node: pyslang.ParameterDeclarationStatementSyntax) -> pyslang.VisitAction:
        param = Parameter.create(node)
        module._params[param.name] = param
        module._param_name_to_decl[param.name] = node
        _update_source_range_for_param(module, node)
        return pyslang.VisitAction.Skip

    @visitor.register
    def _(node: pyslang.PortDeclarationSyntax) -> pyslang.VisitAction:
        port = IOPort.create(node)
        module._ports[port.name] = port
        module._port_name_to_decl[port.name] = node
        _update_source_range_for_port(module, node)
        return pyslang.VisitAction.Skip

    @visitor.register
    def _(
        node: pyslang.DataDeclarationSyntax | pyslang.NetDeclarationSyntax,
    ) -> pyslang.VisitAction:
        signal = {
            pyslang.DataDeclarationSyntax: Reg,
            pyslang.NetDeclarationSyntax: Wire,
        }[type(node)](node.declarators[0].name.valueText, Width.create(node.type))
        module._signals[signal.name] = signal
        module._signal_name_to_decl[signal.name] = node
        _update_source_range_for_signal(module, node)
        return pyslang.VisitAction.Skip

    @visitor.register
    def _(
        node: pyslang.ContinuousAssignSyntax | pyslang.ProceduralBlockSyntax,
    ) -> pyslang.VisitAction:
        module._logics.append(node)
        _update_source_range_for_logic(module, node)
        return pyslang.VisitAction.Skip

    @visitor.register
    def _(node: pyslang.HierarchyInstantiationSyntax) -> pyslang.VisitAction:
        module._instances.append(node)
        _update_source_range_for_instance(module, node)
        return pyslang.VisitAction.Skip

    module._syntax_tree.root.visit(visitor)
    module._module_decl = attrs.module_decl
