"""Pyverilog signal generation for Instance — codegen-only helpers.

This module extracts the pyverilog AST generation logic that Instance
currently carries.  All signal-generating properties and methods are
re-exported here so that new codegen code can use InstanceSignals
instead of coupling directly to Instance.

Existing call-sites that access ``instance.state``, ``instance.done``,
etc. continue to work because Instance still delegates to the same
underlying logic.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal

from pyverilog.vparser.ast import (
    Eq,
    Identifier,
    Node,
    NonblockingSubstitution,
)

from tapa.protocol import (
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_START,
)
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.signal import Reg, Wire
from tapa.verilog.ast.width import Width
from tapa.verilog.util import wire_name

if TYPE_CHECKING:
    from collections.abc import Iterator

    from tapa.instance import Instance


class InstanceSignals:
    """Pyverilog signal generation for an Instance.

    Wraps an :class:`Instance` and provides all the pyverilog AST
    generation methods that were previously on Instance directly.
    """

    def __init__(self, instance: Instance) -> None:
        self.instance = instance

    @property
    def state(self) -> Identifier:
        """State of this instance."""
        return Identifier(f"{self.instance.name}__state")

    def set_state(self, new_state: Node) -> NonblockingSubstitution:
        return NonblockingSubstitution(left=self.state, right=new_state)

    def is_state(self, state: Node) -> Eq:
        return Eq(left=self.state, right=state)

    @property
    def start(self) -> Identifier:
        """The handshake start signal."""
        return Identifier(f"{self.instance.name}__{HANDSHAKE_START}")

    @property
    def done(self) -> Identifier:
        """The handshake done signal."""
        return Identifier(f"{self.instance.name}__{HANDSHAKE_DONE}")

    @property
    def is_done(self) -> Identifier:
        """Signal used to determine the upper-level state."""
        return Identifier(f"{self.instance.name}__is_done")

    @property
    def idle(self) -> Identifier:
        """Whether this instance is idle."""
        return Identifier(f"{self.instance.name}__{HANDSHAKE_IDLE}")

    @property
    def ready(self) -> Identifier:
        """Whether this instance is ready to take new input."""
        return Identifier(f"{self.instance.name}__{HANDSHAKE_READY}")

    @property
    def _public_handshake_tuples(
        self,
    ) -> Iterator[
        tuple[
            type[Reg | Wire],
            Literal["input", "output"],
            str,
        ]
    ]:
        """Public handshake information tuples used for this instance."""
        if self.instance.is_autorun:
            yield (Reg, "output", self.start.name)
        else:
            yield (Wire, "output", self.start.name)
            yield (Wire, "input", wire_name(self.instance.name, HANDSHAKE_READY))
            yield (Wire, "input", wire_name(self.instance.name, HANDSHAKE_DONE))
            yield (Wire, "input", wire_name(self.instance.name, HANDSHAKE_IDLE))

    @property
    def public_handshake_ports(self) -> Iterator[IOPort]:
        """Public handshake IO ports used for this instance."""
        for _, direction, name in self._public_handshake_tuples:
            yield IOPort(direction, name, width=None)

    @property
    def public_handshake_signals(self) -> Iterator[Wire | Reg]:
        """Public handshake signals used for this instance."""
        for signal_type, _, name in self._public_handshake_tuples:
            yield signal_type(name)

    @property
    def all_handshake_signals(self) -> Iterator[Wire | Reg]:
        """All handshake signals used for this instance."""
        yield from self.public_handshake_signals
        if not self.instance.is_autorun:
            yield Reg(self.state.name, Width.create(2))
