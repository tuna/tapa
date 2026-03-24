"""External port objects in TAPA."""

__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from enum import Enum
from functools import lru_cache

from tapa.common.base import Base


class ExternalPort(Base):
    """TAPA external port that is connected to the top level task."""

    class Type(Enum):
        STREAM = 1
        SYNC_MMAP = 2
        ASYNC_MMAP = 3
        SCALAR = 4

    def __init__(
        self,
        name: str | None,
        obj: dict[str, object],
        parent: Base | None = None,
        definition: Base | None = None,
    ) -> None:
        super().__init__(name=name, obj=obj, parent=parent, definition=definition)
        self.global_name = self.name

    @lru_cache(None)
    def get_bitwidth(self) -> int:
        """Returns the bitwidth."""
        assert isinstance(self.obj["width"], int)
        return self.obj["width"]

    @lru_cache(None)
    def get_type(self) -> Type:
        """Returns the type of the external port."""
        cat = str(self.obj["cat"])
        try:
            return {
                "stream": ExternalPort.Type.STREAM,
                "mmap": ExternalPort.Type.SYNC_MMAP,
                "async_mmap": ExternalPort.Type.ASYNC_MMAP,
                "scalar": ExternalPort.Type.SCALAR,
            }[cat]
        except KeyError:
            msg = f'Unknown type "{cat}"'
            raise NotImplementedError(msg) from None
