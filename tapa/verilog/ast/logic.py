"""Parser/vendor-agnostic RTL logic (`Assign`/`Always`) type."""

from typing import NamedTuple


class Assign(NamedTuple):
    lhs: str
    rhs: str

    def __str__(self) -> str:
        return f"assign {self.lhs} = {self.rhs};"


class Always(NamedTuple):
    sens_list: str
    statement: str

    def __str__(self) -> str:
        return f"always @({self.sens_list}) {self.statement}"
