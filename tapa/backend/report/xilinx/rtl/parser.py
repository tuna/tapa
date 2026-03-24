import enum
from typing import TextIO

__all__ = (
    "HierarchicalUtilization",
    "parse_hierarchical_utilization_report",
)


class HierarchicalUtilization:
    """Semantic-agnostic hierarchical utilization."""

    device: str
    parent: "HierarchicalUtilization | None"
    children: list["HierarchicalUtilization"]
    instance: str
    schema: dict[str, int]
    items: tuple[str, ...]

    def __init__(
        self,
        device: str,
        instance: str,
        schema: dict[str, int],
        items: tuple[str, ...],
        parent: "HierarchicalUtilization | None" = None,
    ) -> None:
        if len(schema) != len(items):
            msg = "mismatching schema and items"
            raise TypeError(msg)
        self.device = device
        self.parent = parent
        self.children = []
        if parent is not None:
            parent.children.append(self)
        self.instance = instance
        self.schema = schema
        self.items = items

    def __getitem__(self, key: str) -> str:
        return self.items[self.schema[key]]

    def __str__(self) -> str:
        parent_instance = self.parent.instance if self.parent else None
        lines = ["", f"instance: {self.instance}", f"parent: {parent_instance}"]
        lines.extend(f"{key}: {value}" for key, value in zip(self.schema, self.items))
        return "\n".join(lines)


class _ParseState(enum.Enum):
    PROLOG = 0
    HEADER = 1
    BODY = 2
    EPILOG = 3


def parse_hierarchical_utilization_report(rpt_file: TextIO) -> HierarchicalUtilization:
    """Parse hierarchical utilization report.

    This is a compromise where Vivado won't export structured report from scripts.
    """
    parse_state = _ParseState.PROLOG
    stack: list[HierarchicalUtilization] = []
    device = ""
    schema: dict[str, int] = {}

    for unstripped_line in rpt_file:
        line = unstripped_line.strip()
        words = line.split()
        if len(words) == 4 and words[:3] == ["|", "Device", ":"]:  # noqa: PLR2004
            device = words[3]
            continue
        if set(line) == {"+", "-"}:
            if parse_state == _ParseState.PROLOG:
                parse_state = _ParseState.HEADER
            elif parse_state == _ParseState.HEADER:
                parse_state = _ParseState.BODY
            elif parse_state == _ParseState.BODY:
                parse_state = _ParseState.EPILOG
            else:
                msg = "unexpected table separator line"
                raise ValueError(msg)
            continue

        if parse_state == _ParseState.HEADER:
            instance, cols = get_items(line)
            assert instance.lstrip() == "Instance"
            schema = {x.lstrip(): i for i, x in enumerate(cols)}

        elif parse_state == _ParseState.BODY:
            instance, cols = get_items(line)
            while (len(instance) - len(instance.lstrip(" "))) // 2 < len(stack):
                stack.pop()
            instance = instance.lstrip()
            parent = stack[-1] if stack else None
            stack.append(
                HierarchicalUtilization(device, instance, schema, cols, parent)
            )

    return stack[0]


def get_items(line: str) -> tuple[str, tuple[str, ...]]:
    """Split a table line into instance name and column values."""
    parts = line.strip().strip("|").split("|")
    return parts[0].rstrip(), tuple(p.strip() for p in parts[1:])
