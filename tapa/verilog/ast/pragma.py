"""Parser/vendor-agnostic RTL pragma type."""

from typing import NamedTuple


class Pragma(NamedTuple):
    name: str
    value: str | None = None

    def __str__(self) -> str:
        if self.value is None:
            return f"(* {self.name} *)"
        return f'(* {self.name} = "{self.value}" *)'
