//! Protocol-based port classification using `tapa-protocol` constants.

use serde::{Deserialize, Serialize};
use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST,
    HANDSHAKE_RST_N, HANDSHAKE_START, ISTREAM_SUFFIXES, M_AXI_PREFIX,
    M_AXI_SUFFIXES_COMPACT, OSTREAM_SUFFIXES,
};

use crate::port::Port;

/// Classification of a Verilog port by protocol role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PortClass {
    /// Handshake control port (`ap_clk`, `ap_rst_n`, `ap_start`, etc.).
    Handshake { role: HandshakeRole },
    /// M-AXI port with channel and sub-port info.
    MAxi {
        /// Base port name (e.g., `"m_axi_a"`).
        base: String,
        /// AXI channel (e.g., `"AR"`, `"AW"`, `"R"`, `"W"`, `"B"`).
        channel: String,
        /// Sub-port name within channel (e.g., `"ADDR"`, `"VALID"`).
        sub_port: String,
    },
    /// Input stream port.
    IStream {
        /// Base stream name (e.g., `"data_s"` from `"data_s_dout"`).
        base: String,
        /// Suffix (e.g., `"_dout"`, `"_empty_n"`, `"_read"`).
        suffix: String,
    },
    /// Output stream port.
    OStream {
        /// Base stream name.
        base: String,
        /// Suffix (e.g., `"_din"`, `"_full_n"`, `"_write"`).
        suffix: String,
    },
    /// Port that doesn't match any protocol pattern.
    Unclassified,
}

/// Handshake port roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandshakeRole {
    Clock,
    Reset,
    ResetN,
    Start,
    Done,
    Idle,
    Ready,
}

/// Classify a single port by its name.
pub fn classify_port(port: &Port) -> PortClass {
    let name = &port.name;

    // Check handshake ports first (exact match).
    if let Some(role) = classify_handshake(name) {
        return PortClass::Handshake { role };
    }

    // Check M-AXI ports.
    if let Some(cls) = classify_m_axi(name) {
        return cls;
    }

    // Check input stream ports.
    for &suffix in ISTREAM_SUFFIXES {
        if let Some(base) = name.strip_suffix(suffix) {
            return PortClass::IStream {
                base: base.to_owned(),
                suffix: suffix.to_owned(),
            };
        }
    }

    // Check output stream ports.
    for &suffix in OSTREAM_SUFFIXES {
        if let Some(base) = name.strip_suffix(suffix) {
            return PortClass::OStream {
                base: base.to_owned(),
                suffix: suffix.to_owned(),
            };
        }
    }

    PortClass::Unclassified
}

/// Classify all ports in a module.
pub fn classify_ports(ports: &[Port]) -> Vec<(String, PortClass)> {
    ports
        .iter()
        .map(|p| (p.name.clone(), classify_port(p)))
        .collect()
}

fn classify_handshake(name: &str) -> Option<HandshakeRole> {
    if name == HANDSHAKE_CLK {
        Some(HandshakeRole::Clock)
    } else if name == HANDSHAKE_RST {
        Some(HandshakeRole::Reset)
    } else if name == HANDSHAKE_RST_N {
        Some(HandshakeRole::ResetN)
    } else if name == HANDSHAKE_START {
        Some(HandshakeRole::Start)
    } else if name == HANDSHAKE_DONE {
        Some(HandshakeRole::Done)
    } else if name == HANDSHAKE_IDLE {
        Some(HandshakeRole::Idle)
    } else if name == HANDSHAKE_READY {
        Some(HandshakeRole::Ready)
    } else {
        None
    }
}

fn classify_m_axi(name: &str) -> Option<PortClass> {
    // Optional address-channel attributes not in the compact list.
    const OPTIONAL: &[&str] = &[
        "_ARLOCK", "_ARPROT", "_ARQOS", "_ARCACHE",
        "_AWCACHE", "_AWLOCK", "_AWPROT", "_AWQOS",
    ];

    if !name.starts_with(M_AXI_PREFIX) {
        return None;
    }
    let after_prefix = &name[M_AXI_PREFIX.len()..];

    for &suffix in M_AXI_SUFFIXES_COMPACT.iter().chain(OPTIONAL) {
        if let Some(base_tail) = after_prefix.strip_suffix(suffix) {
            // suffix is like "_ARADDR" -> channel = "AR", sub_port = "ADDR"
            let channel_sub = &suffix[1..]; // skip leading '_'
            let (channel, sub_port) = split_channel_sub(channel_sub)?;
            let base = format!("{M_AXI_PREFIX}{base_tail}");
            return Some(PortClass::MAxi {
                base,
                channel: channel.to_owned(),
                sub_port: sub_port.to_owned(),
            });
        }
    }

    None
}

/// Split a `channel+sub_port` string like `"ARADDR"` into `("AR", "ADDR")`.
fn split_channel_sub(s: &str) -> Option<(&str, &str)> {
    // Known channel prefixes: AR, AW, R, W, B
    for prefix in &["AR", "AW", "B", "R", "W"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return Some((prefix, rest));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::port::Direction;

    use super::*;

    fn make_port(name: &str) -> Port {
        Port {
            name: name.to_owned(),
            direction: Direction::Input,
            width: None,
            pragma: None,
        }
    }

    #[test]
    fn handshake_ports() {
        assert_eq!(
            classify_port(&make_port("ap_clk")),
            PortClass::Handshake {
                role: HandshakeRole::Clock
            }
        );
        assert_eq!(
            classify_port(&make_port("ap_start")),
            PortClass::Handshake {
                role: HandshakeRole::Start
            }
        );
        assert_eq!(
            classify_port(&make_port("ap_done")),
            PortClass::Handshake {
                role: HandshakeRole::Done
            }
        );
    }

    #[test]
    fn istream_ports() {
        let cls = classify_port(&make_port("data_s_dout"));
        assert_eq!(
            cls,
            PortClass::IStream {
                base: "data_s".to_owned(),
                suffix: "_dout".to_owned(),
            }
        );
    }

    #[test]
    fn ostream_ports() {
        let cls = classify_port(&make_port("result_din"));
        assert_eq!(
            cls,
            PortClass::OStream {
                base: "result".to_owned(),
                suffix: "_din".to_owned(),
            }
        );
    }

    #[test]
    fn m_axi_ports() {
        let cls = classify_port(&make_port("m_axi_a_ARADDR"));
        match cls {
            PortClass::MAxi {
                base,
                channel,
                sub_port,
            } => {
                assert_eq!(base, "m_axi_a");
                assert_eq!(channel, "AR");
                assert_eq!(sub_port, "ADDR");
            }
            other @ (PortClass::Handshake { .. }
            | PortClass::IStream { .. }
            | PortClass::OStream { .. }
            | PortClass::Unclassified) => panic!("expected MAxi, got {other:?}"),
        }
    }

    #[test]
    fn unclassified_port() {
        assert_eq!(classify_port(&make_port("scalar")), PortClass::Unclassified);
    }
}
