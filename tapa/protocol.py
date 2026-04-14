"""Pure protocol constants shared across TAPA subsystems.

This module contains string, numeric, tuple, and dict constants that define
the TAPA protocol naming conventions and interface metadata.  It deliberately
has **no** pyverilog or Width imports so that any module can depend on it
without pulling in the Verilog AST layer.
"""

from typing import Literal

__all__ = [
    "CLK_SENS_LIST",
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
    "ISTREAM_SUFFIXES",
    "M_AXI_ADDR_PORTS",
    "M_AXI_PARAM_PREFIX",
    "M_AXI_PARAM_SUFFIXES",
    "M_AXI_PORTS",
    "M_AXI_PORT_WIDTHS",
    "M_AXI_PREFIX",
    "M_AXI_SUFFIXES",
    "M_AXI_SUFFIXES_BY_CHANNEL",
    "M_AXI_SUFFIXES_COMPACT",
    "OSTREAM_SUFFIXES",
    "RTL_SUFFIX",
    "SENS_TYPE",
    "STREAM_DATA_SUFFIXES",
    "STREAM_PORT_DIRECTION",
    "STREAM_PORT_OPPOSITE",
    "STREAM_PORT_WIDTH",
    "S_AXI_NAME",
]

# ---------------------------------------------------------------------------
# Handshake port names
# ---------------------------------------------------------------------------

HANDSHAKE_CLK = "ap_clk"
HANDSHAKE_RST = "ap_rst_n_inv"
HANDSHAKE_RST_N = "ap_rst_n"
HANDSHAKE_START = "ap_start"
HANDSHAKE_DONE = "ap_done"
HANDSHAKE_IDLE = "ap_idle"
HANDSHAKE_READY = "ap_ready"

HANDSHAKE_INPUT_PORTS = (
    HANDSHAKE_CLK,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)
HANDSHAKE_OUTPUT_PORTS = (
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
)

# ---------------------------------------------------------------------------
# Clock / reset / sensitivity
# ---------------------------------------------------------------------------

SENS_TYPE = "posedge"
CLK_SENS_LIST = f"{SENS_TYPE} {HANDSHAKE_CLK}"

# ---------------------------------------------------------------------------
# RTL file extension
# ---------------------------------------------------------------------------

RTL_SUFFIX = ".v"

# ---------------------------------------------------------------------------
# Stream suffixes and port metadata
# ---------------------------------------------------------------------------

ISTREAM_SUFFIXES = (
    "_dout",
    "_empty_n",
    "_read",
)

OSTREAM_SUFFIXES = (
    "_din",
    "_full_n",
    "_write",
)

STREAM_DATA_SUFFIXES = (
    "_dout",
    "_din",
)

STREAM_PORT_DIRECTION: dict[str, Literal["input", "output"]] = {
    "_dout": "input",
    "_empty_n": "input",
    "_read": "output",
    "_din": "output",
    "_full_n": "input",
    "_write": "output",
}

STREAM_PORT_OPPOSITE = {
    "_dout": "_din",
    "_empty_n": "_write",
    "_read": "_full_n",
    "_din": "_dout",
    "_full_n": "_read",
    "_write": "_empty_n",
}

STREAM_PORT_WIDTH = {
    "_dout": 0,
    "_empty_n": 1,
    "_read": 1,
    "_din": 0,
    "_full_n": 1,
    "_write": 1,
}

# ---------------------------------------------------------------------------
# FIFO interface ports
# ---------------------------------------------------------------------------

FIFO_READ_PORTS = (
    "if_dout",
    "if_empty_n",
    "if_read",
    "if_read_ce",
)

FIFO_WRITE_PORTS = (
    "if_din",
    "if_full_n",
    "if_write",
    "if_write_ce",
)

# ---------------------------------------------------------------------------
# AXI naming
# ---------------------------------------------------------------------------

S_AXI_NAME = "s_axi_control"
M_AXI_PREFIX = "m_axi_"

# ---------------------------------------------------------------------------
# M-AXI port widths and channel definitions
# ---------------------------------------------------------------------------

M_AXI_PORT_WIDTHS = {
    "ADDR": 0,
    "BURST": 2,
    "CACHE": 4,
    "DATA": 0,
    "ID": 1,
    "LAST": 1,
    "LEN": 8,
    "LOCK": 1,
    "PROT": 3,
    "QOS": 4,
    "READY": 1,
    "RESP": 2,
    "SIZE": 3,
    "STRB": 0,
    "VALID": 1,
}

M_AXI_ADDR_PORTS: tuple[tuple[str, Literal["input", "output"]], ...] = (
    ("ADDR", "output"),
    ("BURST", "output"),
    ("CACHE", "output"),
    ("ID", "output"),
    ("LEN", "output"),
    ("LOCK", "output"),
    ("PROT", "output"),
    ("QOS", "output"),
    ("READY", "input"),
    ("SIZE", "output"),
    ("VALID", "output"),
)

M_AXI_PORTS: dict[str, tuple[tuple[str, Literal["input", "output"]], ...]] = {
    "AR": M_AXI_ADDR_PORTS,
    "AW": M_AXI_ADDR_PORTS,
    "B": (
        ("ID", "input"),
        ("READY", "output"),
        ("RESP", "input"),
        ("VALID", "input"),
    ),
    "R": (
        ("DATA", "input"),
        ("ID", "input"),
        ("LAST", "input"),
        ("READY", "output"),
        ("RESP", "input"),
        ("VALID", "input"),
    ),
    "W": (
        ("DATA", "output"),
        ("LAST", "output"),
        ("READY", "input"),
        ("STRB", "output"),
        ("VALID", "output"),
    ),
}

M_AXI_SUFFIXES_COMPACT = (
    "_ARADDR",
    "_ARBURST",
    "_ARID",
    "_ARLEN",
    "_ARREADY",
    "_ARSIZE",
    "_ARVALID",
    "_AWADDR",
    "_AWBURST",
    "_AWID",
    "_AWLEN",
    "_AWREADY",
    "_AWSIZE",
    "_AWVALID",
    "_BID",
    "_BREADY",
    "_BRESP",
    "_BVALID",
    "_RDATA",
    "_RID",
    "_RLAST",
    "_RREADY",
    "_RRESP",
    "_RVALID",
    "_WDATA",
    "_WLAST",
    "_WREADY",
    "_WSTRB",
    "_WVALID",
)

M_AXI_SUFFIXES = (
    *M_AXI_SUFFIXES_COMPACT,
    "_ARLOCK",
    "_ARPROT",
    "_ARQOS",
    "_ARCACHE",
    "_AWCACHE",
    "_AWLOCK",
    "_AWPROT",
    "_AWQOS",
)

M_AXI_SUFFIXES_BY_CHANNEL = {
    "AR": {
        "ports": (
            "_ARADDR",
            "_ARBURST",
            "_ARID",
            "_ARLEN",
            "_ARREADY",
            "_ARSIZE",
            "_ARVALID",
            "_ARLOCK",
            "_ARPROT",
            "_ARQOS",
            "_ARCACHE",
        ),
        "valid": "_ARVALID",
        "ready": "_ARREADY",
    },
    "AW": {
        "ports": (
            "_AWADDR",
            "_AWBURST",
            "_AWID",
            "_AWLEN",
            "_AWREADY",
            "_AWSIZE",
            "_AWVALID",
            "_AWLOCK",
            "_AWPROT",
            "_AWQOS",
            "_AWCACHE",
        ),
        "valid": "_AWVALID",
        "ready": "_AWREADY",
    },
    "B": {
        "ports": ("_BID", "_BREADY", "_BRESP", "_BVALID"),
        "valid": "_BVALID",
        "ready": "_BREADY",
    },
    "R": {
        "ports": (
            "_RDATA",
            "_RID",
            "_RLAST",
            "_RREADY",
            "_RRESP",
            "_RVALID",
        ),
        "valid": "_RVALID",
        "ready": "_RREADY",
    },
    "W": {
        "ports": ("_WDATA", "_WLAST", "_WREADY", "_WSTRB", "_WVALID"),
        "valid": "_WVALID",
        "ready": "_WREADY",
    },
}

M_AXI_PARAM_PREFIX = "C_M_AXI_"

M_AXI_PARAM_SUFFIXES = (
    "_ID_WIDTH",
    "_ADDR_WIDTH",
    "_DATA_WIDTH",
    "_PROT_VALUE",
    "_CACHE_VALUE",
    "_WSTRB_WIDTH",
)
