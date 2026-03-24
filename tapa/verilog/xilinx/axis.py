"""AXI Stream code generators for TAPA."""

__all__ = [
    "AXIS_CONSTANTS",
    "AXIS_PORT_WIDTHS",
    "STREAM_TO_AXIS",
    "get_axis_port_width_int",
]

AXIS_PORT_WIDTHS = {
    "TDATA": 0,
    "TLAST": 1,
    "TVALID": 1,
    "TREADY": 1,
    "TKEEP": 0,
}

STREAM_TO_AXIS = {
    "_dout": ["TDATA", "TLAST"],
    "_empty_n": ["TVALID"],
    "_read": ["TREADY"],
    "_din": ["TDATA", "TLAST"],
    "_full_n": ["TREADY"],
    "_write": ["TVALID"],
}

AXIS_CONSTANTS = {
    "TKEEP": 1,
}

AXIS_PORTS = {
    "_TDATA": "data",
    "_TLAST": "data",
    "_TVALID": "valid",
    "_TREADY": "ready",
    "_TKEEP": "data",
}


def get_axis_port_width_int(port: str, data_width: int) -> int:
    width = AXIS_PORT_WIDTHS[port]
    if width == 0:
        width = data_width if port == "TDATA" else data_width // 8
    return width
