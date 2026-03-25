"""Data structure to represent a reset interface."""

from typing import Any, Literal

from tapa.graphir.types.interfaces.feedforward import FeedForwardInterface

_FEEDFORWARD_PORT_COUNT = 2


class FeedForwardResetInterface(FeedForwardInterface):
    """Interface of reset signal.

    Feedforward reset interface for hls kernel
    """

    type: Literal["ff_reset"] = "ff_reset"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: Any) -> None:  # noqa: ANN401
        """Initialize the reset interface."""
        if "rst_port" not in kwargs:
            kwargs["rst_port"] = None
        super().__init__(**kwargs)
        assert len(self.ports) == _FEEDFORWARD_PORT_COUNT

    def __repr__(self) -> str:
        """Represent the interface as a string."""
        return f"ff_rst{self.ports}"

    @staticmethod
    def is_reset() -> bool:
        """Return if the interface is a clock or reset interface."""
        return True

    def get_rst_port(self) -> str:
        """Get the reset port."""
        reset_ports = set(self.ports) - {self.clk_port}
        assert len(reset_ports) == 1
        return reset_ports.pop()
