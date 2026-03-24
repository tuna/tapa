"""Base class of a enum that use its values as its string representation."""

from enum import StrEnum


class StringEnum(StrEnum):
    """Enum with string representation."""

    def __repr__(self) -> str:
        """Use value string as repr (e.g. 'sink' instead of <Role.SINK: 'sink'>)."""
        return repr(str(self))
