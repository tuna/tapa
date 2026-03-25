"""Data structure to represent a false path interface."""

from typing import Any, Literal

from tapa.graphir.types.interfaces.base import BaseInterface


class FalsePathInterface(BaseInterface):
    """Interface where a port is connected to a false path.

    No need to pipeline the connection regardless of distance.
    """

    type: Literal["false_path"] = "false_path"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: Any) -> None:  # noqa: ANN401
        """Preprocessing the input ports."""
        assert not kwargs.get("clk_port")
        assert not kwargs.get("rst_port")
        kwargs["clk_port"] = None
        kwargs["rst_port"] = None
        super().__init__(**kwargs)

    def __repr__(self) -> str:
        """Represent the interface as a string."""
        return f"false{self.ports}"

    def get_data_ports(self) -> tuple[str, ...]:
        """All ports are data ports in feedforward interfaces."""
        return self.ports

    @staticmethod
    def is_clk() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return if the interface is a clock or reset interface."""
        return False

    @staticmethod
    def is_reset() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return if the interface is a clock or reset interface."""
        return False

    @staticmethod
    def is_pipelinable() -> bool:  # type: ignore[reportIncompatibleMethodOverride]
        """Return if the interface is pipelinable."""
        return False
