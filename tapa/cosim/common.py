import re
from collections import defaultdict
from pathlib import Path
from typing import NamedTuple


class AXI:
    def __init__(self, name: str, data_width: int, addr_width: int) -> None:
        self.name = name
        self.data_width = data_width
        self.addr_width = addr_width


class Port(NamedTuple):
    """Port parsed from kernel.xml."""

    name: str
    mode: str  # 'read_only' | 'write_only'
    data_width: int

    @property
    def is_istream(self) -> bool:
        """Returns whether this port is a tapa::istream."""
        return self.mode == "read_only"

    @property
    def is_ostream(self) -> bool:
        """Returns whether this port is a tapa::ostream."""
        return self.mode == "write_only"


_STREAM_QUALIFIER = 4  # address_qualifier value for stream args


class Arg(NamedTuple):
    """Arg parsed from kernel.xml."""

    name: str
    address_qualifier: int  # 0: scalar, 1: mmap, 4: stream
    id: int
    port: Port
    stream_idx: int | None = None

    @property
    def is_scalar(self) -> bool:
        """Returns whether this arg is a scalar."""
        return self.address_qualifier == 0

    @property
    def is_mmap(self) -> bool:
        """Returns whether this arg is a mmap."""
        return self.address_qualifier == 1

    @property
    def is_stream(self) -> bool:
        """Returns whether this arg is a stream."""
        return self.address_qualifier == _STREAM_QUALIFIER

    @property
    def qualified_name(self) -> str:
        """Returns the qualified name of this arg which was manipulated by HLS."""
        if not self.is_stream:
            return self.name
        return (
            f"{self.name}_s"
            if self.stream_idx is None
            else f"{self.name}_{self.stream_idx}"
        )

    @property
    def peek_qualified_name(self) -> str | None:
        """Returns the name to access the peek port of this arg."""
        if not self.is_stream:
            return None
        return (
            f"{self.name}_peek"
            if self.stream_idx is None
            else f"{self.name}_peek_{self.stream_idx}"
        )


MAX_AXI_BRAM_ADDR_WIDTH = 32


def output_data_path(input_path: str) -> str:
    p = Path(input_path)
    return str(p.with_name(f"{p.stem}_out.bin"))


def parse_register_addr(ctrl_unit_path: str) -> dict[str, list[str]]:
    """Parses register addresses from the given s_axi_control.v file.

    Parses the comments in s_axi_control.v to get the register addresses for each
    argument.
    """
    with open(ctrl_unit_path, encoding="utf-8") as fp:
        comments = [line for line in fp if line.strip().startswith("//")]

    arg_to_reg_addrs: dict[str, list[str]] = defaultdict(list)
    for line in comments:
        if " 0x" not in line or "Data signal" not in line:
            continue
        match = re.search(r"(0x\w+) : Data signal of (\w+)", line)
        if match:
            arg_to_reg_addrs[match.group(2)].append(match.group(1).replace("0x", "'h"))

    return dict(arg_to_reg_addrs)
