"""Interconnect definition object in TAPA."""

from enum import Enum
from functools import lru_cache

from tapa.common.base import Base


class InterconnectDefinition(Base):
    """TAPA local interconnect definition."""

    class Type(Enum):
        STREAM = 1
        SYNC_MMAP = 2
        ASYNC_MMAP = 3
        SCALAR = 4

    @lru_cache(None)
    def get_depth(self) -> int:
        """Return the depth of the local interconnect."""
        if self.get_type() != InterconnectDefinition.Type.STREAM:
            msg = "Local interconnects other than streams are not implemented yet."
            raise NotImplementedError(msg)
        assert isinstance(self.obj["depth"], int)
        return self.obj["depth"]

    @staticmethod
    def get_type() -> "InterconnectDefinition.Type":
        """Return the type of the local interconnect."""
        return InterconnectDefinition.Type.STREAM
