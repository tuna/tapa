"""Data structure to represent a clock interface."""

from typing import Any, Literal

from tapa.graphir.types.interfaces.falsepath import FalsePathInterface


class ClockInterface(FalsePathInterface):
    """Interface of clock signal.

    No need to pipeline the connection regardless of distance since clock is falsepath.
    """

    type: Literal["clock"] = "clock"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: Any) -> None:  # noqa: ANN401
        """Preprocessing the input ports."""
        super().__init__(**kwargs)
        assert len(self.ports) == 1

    def __repr__(self) -> str:
        """Represent the interface as a string."""
        return f"clk({self.ports[0]}, role={self.role})"

    @staticmethod
    def is_clk() -> bool:
        """Return if the interface is a clock or reset interface."""
        return True
