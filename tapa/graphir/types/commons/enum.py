"""Base class of a enum that use its values as its string representation."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from enum import StrEnum


class StringEnum(StrEnum):
    """Enum with string representation."""

    def __repr__(self) -> str:
        """Use value string as repr (e.g. 'sink' instead of <Role.SINK: 'sink'>)."""
        return repr(str(self))
