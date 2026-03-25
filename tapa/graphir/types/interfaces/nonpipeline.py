"""Data structure to represent a clock interface."""

from typing import Literal

from tapa.graphir.types.interfaces.base import BaseInterface


class NonPipelineInterface(BaseInterface):
    """Interface with ports that must not be pipelined."""

    clk_port: None = None  # type: ignore[reportIncompatibleVariableOverride]
    rst_port: None = None  # type: ignore[reportIncompatibleVariableOverride]
    role: BaseInterface.InterfaceRole
    type: Literal["non_pipeline"] = "non_pipeline"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: object) -> None:
        """Preprocessing the input ports."""
        # Check that ports excludes clk_port and rst_port.
        assert "clk_port" not in kwargs or kwargs["clk_port"] is None
        assert "rst_port" not in kwargs or kwargs["rst_port"] is None

        kwargs["clk_port"] = None
        kwargs["rst_port"] = None

        # default role is TBD
        kwargs["role"] = kwargs.get("role", BaseInterface.InterfaceRole.TBD)

        if not kwargs["ports"]:
            msg = "Interface must have at least one port."
            raise RuntimeError(msg)

        super().__init__(**kwargs)  # type: ignore[arg-type]

    def __repr__(self) -> str:
        """Represent the interface as a string."""
        return f"np{self.ports}"

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


class UnknownInterface(NonPipelineInterface):
    """Auto-added non-pipeline interface.

    We should distinguish between user-defined / report inferred non-pps from the auto-
    inferred ones on uncovered ports.
    """

    type: Literal["unknown"] = "unknown"  # type: ignore[reportIncompatibleVariableOverride]

    def __init__(self, **kwargs: object) -> None:
        """Prevent pyright false positive warning of missing args."""
        kwargs["role"] = kwargs.get("role", BaseInterface.InterfaceRole.TBD)
        super().__init__(**kwargs)


class TAPAPeekInterface(NonPipelineInterface):
    """TAPA peek interface."""

    type: Literal["tapa_peek"] = "tapa_peek"  # type: ignore[reportIncompatibleVariableOverride]
