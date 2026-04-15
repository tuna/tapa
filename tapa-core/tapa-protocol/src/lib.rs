//! TAPA protocol constants.
//!
//! Single source of truth for handshake, stream, FIFO, and M-AXI naming
//! conventions shared across the TAPA toolchain.  This crate has zero
//! dependencies and mirrors `tapa/protocol.py` exactly.

use std::collections::HashMap;
use std::sync::LazyLock;

// ── Handshake port names ────────────────────────────────────────────

pub const HANDSHAKE_CLK: &str = "ap_clk";
pub const HANDSHAKE_RST: &str = "ap_rst_n_inv";
pub const HANDSHAKE_RST_N: &str = "ap_rst_n";
pub const HANDSHAKE_START: &str = "ap_start";
pub const HANDSHAKE_DONE: &str = "ap_done";
pub const HANDSHAKE_IDLE: &str = "ap_idle";
pub const HANDSHAKE_READY: &str = "ap_ready";

pub const HANDSHAKE_INPUT_PORTS: &[&str] = &[HANDSHAKE_CLK, HANDSHAKE_RST_N, HANDSHAKE_START];
pub const HANDSHAKE_OUTPUT_PORTS: &[&str] = &[HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY];

// ── Clock / reset / sensitivity ─────────────────────────────────────

pub const SENS_TYPE: &str = "posedge";

/// `"posedge ap_clk"` — used as a sensitivity list in always blocks.
pub static CLK_SENS_LIST: LazyLock<String> =
    LazyLock::new(|| format!("{SENS_TYPE} {HANDSHAKE_CLK}"));

// ── RTL file extension ──────────────────────────────────────────────

pub const RTL_SUFFIX: &str = ".v";

// ── Stream port suffixes ────────────────────────────────────────────

pub const ISTREAM_SUFFIXES: &[&str] = &["_dout", "_empty_n", "_read"];
pub const OSTREAM_SUFFIXES: &[&str] = &["_din", "_full_n", "_write"];
pub const STREAM_DATA_SUFFIXES: &[&str] = &["_dout", "_din"];

/// Port-name suffix → wire direction (`"input"` or `"output"`).
pub static STREAM_PORT_DIRECTION: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        HashMap::from([
            ("_dout", "input"),
            ("_empty_n", "input"),
            ("_read", "output"),
            ("_din", "output"),
            ("_full_n", "input"),
            ("_write", "output"),
        ])
    });

/// Each stream suffix mapped to its opposite-side counterpart.
pub static STREAM_PORT_OPPOSITE: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        HashMap::from([
            ("_dout", "_din"),
            ("_din", "_dout"),
            ("_empty_n", "_write"),
            ("_write", "_empty_n"),
            ("_read", "_full_n"),
            ("_full_n", "_read"),
        ])
    });

/// Bit width for each stream suffix.  `0` means width is determined by
/// the data type (variable).
pub static STREAM_PORT_WIDTH: LazyLock<HashMap<&'static str, u32>> = LazyLock::new(|| {
    HashMap::from([
        ("_dout", 0),
        ("_din", 0),
        ("_empty_n", 1),
        ("_full_n", 1),
        ("_read", 1),
        ("_write", 1),
    ])
});

// ── FIFO interface ports ────────────────────────────────────────────

pub const FIFO_READ_PORTS: &[&str] = &["if_dout", "if_empty_n", "if_read", "if_read_ce"];
pub const FIFO_WRITE_PORTS: &[&str] = &["if_din", "if_full_n", "if_write", "if_write_ce"];

// ── AXI naming prefixes ─────────────────────────────────────────────

pub const S_AXI_NAME: &str = "s_axi_control";
pub const M_AXI_PREFIX: &str = "m_axi_";
pub const M_AXI_PARAM_PREFIX: &str = "C_M_AXI_";

// ── M-AXI port widths ───────────────────────────────────────────────

/// Default bit width for each M-AXI sub-port.  `0` means the width is
/// parameterised (ADDR, DATA) or derived (STRB = DATA / 8).
pub static M_AXI_PORT_WIDTHS: LazyLock<HashMap<&'static str, u32>> = LazyLock::new(|| {
    HashMap::from([
        ("ADDR", 0),
        ("BURST", 2),
        ("CACHE", 4),
        ("DATA", 0),
        ("ID", 1),
        ("LAST", 1),
        ("LEN", 8),
        ("LOCK", 1),
        ("PROT", 3),
        ("QOS", 4),
        ("READY", 1),
        ("RESP", 2),
        ("SIZE", 3),
        ("STRB", 0),
        ("VALID", 1),
    ])
});

/// Direction: `"output"` means the master drives the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDir {
    Input,
    Output,
}

/// A single (sub-port name, direction) entry inside an AXI channel.
pub type AxiPortEntry = (&'static str, PortDir);

/// Address-channel ports shared by AR and AW channels.
pub const M_AXI_ADDR_PORTS: &[AxiPortEntry] = &[
    ("ADDR", PortDir::Output),
    ("BURST", PortDir::Output),
    ("CACHE", PortDir::Output),
    ("ID", PortDir::Output),
    ("LEN", PortDir::Output),
    ("LOCK", PortDir::Output),
    ("PROT", PortDir::Output),
    ("QOS", PortDir::Output),
    ("READY", PortDir::Input),
    ("SIZE", PortDir::Output),
    ("VALID", PortDir::Output),
];

/// All five M-AXI channels → their sub-port lists.
pub static M_AXI_PORTS: LazyLock<HashMap<&'static str, &'static [AxiPortEntry]>> =
    LazyLock::new(|| {
        let b: &'static [AxiPortEntry] = &[
            ("ID", PortDir::Input),
            ("READY", PortDir::Output),
            ("RESP", PortDir::Input),
            ("VALID", PortDir::Input),
        ];
        let r: &'static [AxiPortEntry] = &[
            ("DATA", PortDir::Input),
            ("ID", PortDir::Input),
            ("LAST", PortDir::Input),
            ("READY", PortDir::Output),
            ("RESP", PortDir::Input),
            ("VALID", PortDir::Input),
        ];
        let w: &'static [AxiPortEntry] = &[
            ("DATA", PortDir::Output),
            ("LAST", PortDir::Output),
            ("READY", PortDir::Input),
            ("STRB", PortDir::Output),
            ("VALID", PortDir::Output),
        ];
        HashMap::from([
            ("AR", M_AXI_ADDR_PORTS),
            ("AW", M_AXI_ADDR_PORTS),
            ("B", b),
            ("R", r),
            ("W", w),
        ])
    });

// ── M-AXI suffixes ──────────────────────────────────────────────────

/// Compact suffix set (29 entries) — no optional address-channel attributes.
pub const M_AXI_SUFFIXES_COMPACT: &[&str] = &[
    "_ARADDR", "_ARBURST", "_ARID", "_ARLEN", "_ARREADY", "_ARSIZE", "_ARVALID",
    "_AWADDR", "_AWBURST", "_AWID", "_AWLEN", "_AWREADY", "_AWSIZE", "_AWVALID",
    "_BID", "_BREADY", "_BRESP", "_BVALID",
    "_RDATA", "_RID", "_RLAST", "_RREADY", "_RRESP", "_RVALID",
    "_WDATA", "_WLAST", "_WREADY", "_WSTRB", "_WVALID",
];

/// Full suffix set (37 entries) — compact + 8 optional address-channel attributes.
pub static M_AXI_SUFFIXES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut v = M_AXI_SUFFIXES_COMPACT.to_vec();
    v.extend_from_slice(&[
        "_ARLOCK", "_ARPROT", "_ARQOS", "_ARCACHE",
        "_AWCACHE", "_AWLOCK", "_AWPROT", "_AWQOS",
    ]);
    v
});

/// Per-channel suffix groupings with valid/ready markers.
pub struct AxiChannelInfo {
    pub ports: &'static [&'static str],
    pub valid: &'static str,
    pub ready: &'static str,
}

pub static M_AXI_SUFFIXES_BY_CHANNEL: LazyLock<HashMap<&'static str, AxiChannelInfo>> =
    LazyLock::new(|| {
        HashMap::from([
            (
                "AR",
                AxiChannelInfo {
                    ports: &[
                        "_ARADDR", "_ARBURST", "_ARID", "_ARLEN", "_ARREADY",
                        "_ARSIZE", "_ARVALID", "_ARLOCK", "_ARPROT", "_ARQOS", "_ARCACHE",
                    ],
                    valid: "_ARVALID",
                    ready: "_ARREADY",
                },
            ),
            (
                "AW",
                AxiChannelInfo {
                    ports: &[
                        "_AWADDR", "_AWBURST", "_AWID", "_AWLEN", "_AWREADY",
                        "_AWSIZE", "_AWVALID", "_AWLOCK", "_AWPROT", "_AWQOS", "_AWCACHE",
                    ],
                    valid: "_AWVALID",
                    ready: "_AWREADY",
                },
            ),
            (
                "B",
                AxiChannelInfo {
                    ports: &["_BID", "_BREADY", "_BRESP", "_BVALID"],
                    valid: "_BVALID",
                    ready: "_BREADY",
                },
            ),
            (
                "R",
                AxiChannelInfo {
                    ports: &["_RDATA", "_RID", "_RLAST", "_RREADY", "_RRESP", "_RVALID"],
                    valid: "_RVALID",
                    ready: "_RREADY",
                },
            ),
            (
                "W",
                AxiChannelInfo {
                    ports: &["_WDATA", "_WLAST", "_WREADY", "_WSTRB", "_WVALID"],
                    valid: "_WVALID",
                    ready: "_WREADY",
                },
            ),
        ])
    });

// ── M-AXI parameter suffixes ────────────────────────────────────────

pub const M_AXI_PARAM_SUFFIXES: &[&str] = &[
    "_ID_WIDTH",
    "_ADDR_WIDTH",
    "_DATA_WIDTH",
    "_PROT_VALUE",
    "_CACHE_VALUE",
    "_WSTRB_WIDTH",
];

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_constants() {
        assert_eq!(HANDSHAKE_CLK, "ap_clk", "HANDSHAKE_CLK");
        assert_eq!(HANDSHAKE_RST, "ap_rst_n_inv", "HANDSHAKE_RST");
        assert_eq!(HANDSHAKE_RST_N, "ap_rst_n", "HANDSHAKE_RST_N");
        assert_eq!(HANDSHAKE_START, "ap_start", "HANDSHAKE_START");
        assert_eq!(HANDSHAKE_DONE, "ap_done", "HANDSHAKE_DONE");
        assert_eq!(HANDSHAKE_IDLE, "ap_idle", "HANDSHAKE_IDLE");
        assert_eq!(HANDSHAKE_READY, "ap_ready", "HANDSHAKE_READY");
    }

    #[test]
    fn handshake_port_groups() {
        assert_eq!(HANDSHAKE_INPUT_PORTS.len(), 3, "input ports count");
        assert_eq!(HANDSHAKE_OUTPUT_PORTS.len(), 3, "output ports count");
        assert_eq!(HANDSHAKE_INPUT_PORTS[0], HANDSHAKE_CLK, "first input port");
    }

    #[test]
    fn clk_sens_list() {
        assert_eq!(*CLK_SENS_LIST, "posedge ap_clk", "CLK_SENS_LIST");
    }

    #[test]
    fn stream_suffixes() {
        assert_eq!(ISTREAM_SUFFIXES, &["_dout", "_empty_n", "_read"], "ISTREAM_SUFFIXES");
        assert_eq!(OSTREAM_SUFFIXES, &["_din", "_full_n", "_write"], "OSTREAM_SUFFIXES");
        assert_eq!(STREAM_DATA_SUFFIXES, &["_dout", "_din"], "STREAM_DATA_SUFFIXES");
    }

    #[test]
    fn stream_port_metadata() {
        assert_eq!(STREAM_PORT_DIRECTION.len(), 6, "direction map size");
        assert_eq!(STREAM_PORT_DIRECTION["_dout"], "input", "dout direction");
        assert_eq!(STREAM_PORT_DIRECTION["_din"], "output", "din direction");

        assert_eq!(STREAM_PORT_OPPOSITE["_dout"], "_din", "dout opposite");
        assert_eq!(STREAM_PORT_OPPOSITE["_empty_n"], "_write", "empty_n opposite");

        assert_eq!(STREAM_PORT_WIDTH["_dout"], 0, "dout width");
        assert_eq!(STREAM_PORT_WIDTH["_read"], 1, "read width");
    }

    #[test]
    fn fifo_ports() {
        assert_eq!(FIFO_READ_PORTS.len(), 4, "FIFO read ports count");
        assert_eq!(FIFO_WRITE_PORTS.len(), 4, "FIFO write ports count");
        assert_eq!(FIFO_READ_PORTS[0], "if_dout", "first FIFO read port");
        assert_eq!(FIFO_WRITE_PORTS[0], "if_din", "first FIFO write port");
    }

    #[test]
    fn axi_naming() {
        assert_eq!(S_AXI_NAME, "s_axi_control", "S_AXI_NAME");
        assert_eq!(M_AXI_PREFIX, "m_axi_", "M_AXI_PREFIX");
        assert_eq!(M_AXI_PARAM_PREFIX, "C_M_AXI_", "M_AXI_PARAM_PREFIX");
    }

    #[test]
    fn m_axi_port_widths() {
        assert_eq!(M_AXI_PORT_WIDTHS["ADDR"], 0, "ADDR width");
        assert_eq!(M_AXI_PORT_WIDTHS["BURST"], 2, "BURST width");
        assert_eq!(M_AXI_PORT_WIDTHS["CACHE"], 4, "CACHE width");
        assert_eq!(M_AXI_PORT_WIDTHS["DATA"], 0, "DATA width");
        assert_eq!(M_AXI_PORT_WIDTHS["ID"], 1, "ID width");
        assert_eq!(M_AXI_PORT_WIDTHS["LEN"], 8, "LEN width");
        assert_eq!(M_AXI_PORT_WIDTHS["VALID"], 1, "VALID width");
        assert_eq!(M_AXI_PORT_WIDTHS.len(), 15, "port widths map size");
    }

    #[test]
    fn m_axi_ports_channels() {
        assert_eq!(M_AXI_PORTS.len(), 5, "channel count");
        assert!(M_AXI_PORTS.contains_key("AR"), "has AR");
        assert!(M_AXI_PORTS.contains_key("AW"), "has AW");
        assert!(M_AXI_PORTS.contains_key("B"), "has B");
        assert!(M_AXI_PORTS.contains_key("R"), "has R");
        assert!(M_AXI_PORTS.contains_key("W"), "has W");
        assert_eq!(M_AXI_ADDR_PORTS.len(), 11, "addr ports count");
    }

    #[test]
    fn m_axi_suffixes_counts() {
        assert_eq!(M_AXI_SUFFIXES_COMPACT.len(), 29, "compact suffix count");
        assert_eq!(M_AXI_SUFFIXES.len(), 37, "full suffix count");
    }

    #[test]
    fn m_axi_suffixes_by_channel_structure() {
        assert_eq!(M_AXI_SUFFIXES_BY_CHANNEL.len(), 5, "channel count");
        let ar = &M_AXI_SUFFIXES_BY_CHANNEL["AR"];
        assert_eq!(ar.ports.len(), 11, "AR ports count");
        assert_eq!(ar.valid, "_ARVALID", "AR valid");
        assert_eq!(ar.ready, "_ARREADY", "AR ready");
    }

    #[test]
    fn m_axi_param_suffixes() {
        assert_eq!(M_AXI_PARAM_SUFFIXES.len(), 6, "param suffix count");
        assert_eq!(M_AXI_PARAM_SUFFIXES[0], "_ID_WIDTH", "first param suffix");
    }

    #[test]
    fn rtl_suffix() {
        assert_eq!(RTL_SUFFIX, ".v", "RTL_SUFFIX");
    }
}
