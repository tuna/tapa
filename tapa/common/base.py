"""Base class for TAPA objects."""

from __future__ import annotations

import copy
import re

_ARRAY_NAME_RE = re.compile(r"(\w+)\[(\d+)\]")


def _match_array_name(name: str) -> tuple[str, int] | None:
    match = _ARRAY_NAME_RE.fullmatch(name)
    return (match[1], int(match[2])) if match is not None else None


def _array_name(name: str, idx: int) -> str:
    return f"{name}[{idx}]"


class Base:
    """Describes a TAPA base object.

    Attributes:
      name: Name of the object, which is locally unique in its parent.
      obj: The JSON dict object of the TAPA object.
      parent: The TAPA object that has this object nested in.
      definition: The TAPA definition object of this object, or self.
      global_name: Globally descriptive name of this object in a Graph.
    """

    uuid_counter = 0

    def __init__(
        self,
        name: str | None,
        obj: dict[str, object],
        parent: Base | None = None,
        definition: Base | None = None,
    ) -> None:
        self.obj = copy.deepcopy(obj)
        self.name = name
        self.parent = parent
        self.global_name = self._generate_global_name()
        self.definition = self if definition is None else definition

    def to_dict(self) -> dict[str, object]:
        """Return the TAPA object as a JSON description."""
        return copy.deepcopy(self.obj)

    def _generate_global_name(self) -> str:
        """Return the global name for an object."""
        if (
            self.name is not None
            and (match := _match_array_name(self.name)) is not None
        ):
            return _array_name(
                self._generate_global_name_without_sub(match[0]), match[1]
            )
        return self._generate_global_name_without_sub(self.name)

    def _generate_global_name_without_sub(self, name: str | None) -> str:
        """Returns the global name for a name without an array subscript."""
        from tapa.common.graph import Graph  # noqa: PLC0415

        if type(self.parent) is Graph:
            assert name is not None
            return name

        if self.parent is not None and self.parent.global_name is not None:
            assert name is not None
            return f"{name}_{self.parent.global_name}"

        Base.uuid_counter += 1
        if self.name is not None:
            assert name is not None
            return f"{name}_{Base.uuid_counter}"

        return f"object_{Base.uuid_counter}"
