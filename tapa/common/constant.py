"""Constant class for TAPA constant passed as an argument to a task."""

from tapa.common.base import Base


class Constant(Base):
    """TAPA constant passed as an argument to a task."""

    def __init__(
        self,
        name: str | None,
        obj: dict[str, object],
        parent: Base | None = None,
        definition: Base | None = None,
    ) -> None:
        super().__init__(name=name, obj=obj, parent=parent, definition=definition)
        self.global_name = self.name
