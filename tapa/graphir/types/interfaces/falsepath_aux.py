"""Data structure to represent a reset interface."""

from typing import Any, Literal

from tapa.graphir.types.interfaces.falsepath import FalsePathInterface


class AuxInterface(FalsePathInterface):
    """Interface of aux module signal.

    No need to pipeline the connection regardless of distance since reset is falsepath.
    """

    type: Literal["aux"] = "aux"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: Any) -> None:  # noqa: ANN401
        """Initialize the reset interface."""
        super().__init__(**kwargs)
        assert len(self.ports) == 1

    def __repr__(self) -> str:
        """Represent the interface as a string."""
        return f"aux({self.ports[0]})"
