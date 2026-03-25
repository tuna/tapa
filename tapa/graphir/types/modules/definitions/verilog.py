"""Data structure to represent a verilog module definition."""

import re
from collections.abc import Generator
from typing import Literal

from pydantic import model_validator

from tapa.graphir.types.commons.name import NamedModel
from tapa.graphir.types.modules.definitions.base import BaseModuleDefinition


class VerilogModuleDefinition(BaseModuleDefinition):
    """A definition of a computation module written in Verilog.

    Attributes:
        verilog (str): The verilog source code of the module.

    Examples:
        >>> import json
        >>> print(
        ...     json.dumps(
        ...         VerilogModuleDefinition(
        ...             name="empty_module",
        ...             hierarchical_name=None,
        ...             parameters=[],
        ...             ports=[],
        ...             verilog="",
        ...             submodules_module_names=(),
        ...         ).model_dump()
        ...     )
        ... )
        ... # doctest: +NORMALIZE_WHITESPACE
        {"name": "empty_module", "hierarchical_name": null,
        "module_type": "verilog_module", "parameters": [], "ports": [],
        "metadata": null, "verilog": "", "submodules_module_names": []}

        >>> VerilogModuleDefinition.model_validate_json(
        ...     '''{
        ...     "name": "nested_module",
        ...     "hierarchical_name": null,
        ...     "parameters": [
        ...         {"name": "a", "hierarchical_name": null, "expr": []},
        ...         {"name": "b", "hierarchical_name": null,
        ...          "expr": [{"type": "lit", "repr": "1"}]}
        ...     ],
        ...     "ports": [
        ...         {"name": "a", "hierarchical_name": null,
        ...          "type": "input wire", "range": null},
        ...         {"name": "b", "hierarchical_name": null,
        ...          "type": "output wire", "range": null},
        ...         {"name": "c", "hierarchical_name": null,
        ...          "type": "input wire", "range": null}
        ...     ],
        ...     "verilog": "",
        ...     "submodules_module_names": []
        ... }'''
        ... )
        ... # doctest: +NORMALIZE_WHITESPACE
        VerilogModuleDefinition(name='nested_module',
            hierarchical_name=HierarchicalName(root=None),
            module_type='verilog_module',
            parameters=(ModuleParameter(name='a',
                    hierarchical_name=HierarchicalName(root=None),
                    expr=None, range=None),
                ModuleParameter(name='b',
                    hierarchical_name=HierarchicalName(root=None),
                    expr='1', range=None)),
            ports=(ModulePort(name='a',
                    hierarchical_name=HierarchicalName(root=None),
                    type='input wire', range=None),
                ModulePort(name='b',
                    hierarchical_name=HierarchicalName(root=None),
                    type='output wire', range=None),
                ModulePort(name='c',
                    hierarchical_name=HierarchicalName(root=None),
                    type='input wire', range=None)),
            metadata=None,
            verilog='', submodules_module_names=())
    """

    module_type: Literal["verilog_module"] = "verilog_module"  # type: ignore[reportIncompatibleVariableOverride]

    verilog: str
    submodules_module_names: tuple[str, ...]

    def get_all_named(self) -> Generator[NamedModel]:
        """Yields all the named objects in the namespace."""
        yield from self.ports
        yield from self.parameters

    @model_validator(mode="before")
    @classmethod
    def _sort_verilog_module_fields(cls, data: dict) -> dict:
        """Sort the tuple arguments by name."""
        cls.sort_tuple_field(data, "submodules_module_names")
        return data

    @staticmethod
    def is_leaf_module() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return True: verilog modules are always leaf modules."""
        return True

    def get_submodules_module_names(self) -> tuple[str, ...]:
        """Return the set of submodule module names."""
        return self.submodules_module_names

    @staticmethod
    def is_internal_module() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return False: verilog modules are not internal modules."""
        return False

    def module_name_updated(self, new_name: str) -> "VerilogModuleDefinition":
        """Update the module name and the verilog."""
        name_pattern = rf"\b{self.name}\b"
        matches = re.findall(name_pattern, self.verilog)
        if len(matches) != 1:
            msg = (
                f"Expected exactly one match for keyword '{self.name}' "
                f"in Verilog code, but found {len(matches)} matches."
            )
            raise NotImplementedError(msg)
        return self.updated(
            name=new_name,
            verilog=re.sub(name_pattern, new_name, self.verilog),
        )
