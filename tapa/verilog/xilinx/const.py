"""Constants used in TAPA code generation."""

from pyverilog.vparser.ast import (
    Identifier,
    IntConst,
    Unot,
)

from tapa.protocol import (
    CLK_SENS_LIST,
    FIFO_READ_PORTS,
    FIFO_WRITE_PORTS,
    HANDSHAKE_CLK,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_INPUT_PORTS,
    HANDSHAKE_OUTPUT_PORTS,
    HANDSHAKE_READY,
    HANDSHAKE_RST,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    RTL_SUFFIX,
    SENS_TYPE,
    STREAM_DATA_SUFFIXES,
    STREAM_PORT_DIRECTION,
    STREAM_PORT_OPPOSITE,
    STREAM_PORT_WIDTH,
)
from tapa.verilog.ast.width import Width

__all__ = [
    "CLK",
    "CLK_SENS_LIST",
    "DONE",
    "FALSE",
    "FIFO_READ_PORTS",
    "FIFO_WRITE_PORTS",
    "HANDSHAKE_CLK",
    "HANDSHAKE_DONE",
    "HANDSHAKE_IDLE",
    "HANDSHAKE_INPUT_PORTS",
    "HANDSHAKE_OUTPUT_PORTS",
    "HANDSHAKE_READY",
    "HANDSHAKE_RST",
    "HANDSHAKE_RST_N",
    "HANDSHAKE_START",
    "IDLE",
    "ISTREAM_SUFFIXES",
    "OSTREAM_SUFFIXES",
    "READY",
    "RST",
    "RST_N",
    "RTL_SUFFIX",
    "SENS_TYPE",
    "START",
    "STATE",
    "STREAM_DATA_SUFFIXES",
    "STREAM_PORT_DIRECTION",
    "STREAM_PORT_OPPOSITE",
    "STREAM_PORT_WIDTH",
    "TRUE",
    "get_stream_width",
]

START = Identifier(HANDSHAKE_START)
DONE = Identifier(HANDSHAKE_DONE)
IDLE = Identifier(HANDSHAKE_IDLE)
READY = Identifier(HANDSHAKE_READY)
TRUE = IntConst("1'b1")
FALSE = IntConst("1'b0")
CLK = Identifier(HANDSHAKE_CLK)
RST_N = Identifier(HANDSHAKE_RST_N)
RST = Unot(RST_N)
STATE = Identifier("tapa_state")


def get_stream_width(port: str, data_width: int) -> Width | None:
    width = STREAM_PORT_WIDTH[port] or (data_width + 1)  # 0 => data+eot
    return None if width == 1 else Width.create(width)
