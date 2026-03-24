"""External port objects in TAPA."""

from enum import Enum
from functools import lru_cache

from tapa.common.base import Base

_CAT_TO_TYPE = {
    "stream": "STREAM",
    "mmap": "SYNC_MMAP",
    "async_mmap": "ASYNC_MMAP",
    "scalar": "SCALAR",
}


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
        if cat not in _CAT_TO_TYPE:
            msg = f'Unknown type "{cat}"'
            raise NotImplementedError(msg)
        return ExternalPort.Type[_CAT_TO_TYPE[cat]]
