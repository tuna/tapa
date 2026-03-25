"""Data structure to represent a stub module definition."""

from collections.abc import Generator
from typing import Literal

from tapa.graphir.types.commons.name import NamedModel
from tapa.graphir.types.modules.definitions.base import BaseModuleDefinition


class StubModuleDefinition(BaseModuleDefinition):
    """A definition of a stub module with only interface information.

    Examples:
        >>> import json
        >>> print(
        ...     json.dumps(
        ...         StubModuleDefinition(
        ...             name="empty_module",
        ...             hierarchical_name=None,
        ...             parameters=[],
        ...             ports=[],
        ...         ).model_dump()
        ...     )
        ... )
        ... # doctest: +NORMALIZE_WHITESPACE
        {"name": "empty_module", "hierarchical_name": null,
         "module_type": "stub_module", "parameters": [], "ports": [], "metadata": null}
    """

    module_type: Literal["stub_module"] = "stub_module"  # type: ignore[reportIncompatibleVariableOverride]

    def get_all_named(self) -> Generator[NamedModel]:
        """Yields all the named objects in the namespace."""
        yield from self.ports
        yield from self.parameters

    @staticmethod
    def get_submodules_module_names() -> tuple[str, ...]:  # type: ignore[reportIncompatibleMethodOverride]
        """Return empty tuple: stub modules have no submodules."""
        return ()

    @staticmethod
    def is_leaf_module() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return True: stub modules are always leaf modules."""
        return True

    @staticmethod
    def is_internal_module() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return False: stub modules are not internal modules."""
        return False
