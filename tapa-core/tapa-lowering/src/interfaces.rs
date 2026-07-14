//! Interface (`ifaces`) construction for the exported project.

use std::collections::BTreeMap;

use tapa_graphir::{interface::AnyInterface, AnyModuleDefinition};
use tapa_topology::program::Program;

use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST_N,
    HANDSHAKE_START, ISTREAM_SUFFIXES, M_AXI_PREFIX, OSTREAM_SUFFIXES, S_AXI_LITE_CTRL_PORTS,
    S_AXI_NAME,
};

/// Build interfaces for all module definitions: dedicated builders for
/// FIFO, `reset_inverter`, FSM, `ctrl_s_axi`, and slot/top tasks. Each
/// builder produces the correct interface types with valid/ready ports.
#[allow(
    clippy::too_many_lines,
    reason = "sequential interface assembly per module type"
)]
pub fn build_interfaces(
    module_defs: &[AnyModuleDefinition],
    program: &Program,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<AnyInterface>> {
    use tapa_graphir::interface::InterfaceBase;
    let mut ifaces = BTreeMap::new();

    let make_hs = |ports: Vec<String>,
                   clk: Option<&str>,
                   rst: Option<&str>,
                   valid: &str,
                   ready: &str|
     -> AnyInterface {
        AnyInterface::HandShake {
            base: InterfaceBase {
                clk_port: clk.map(str::to_owned),
                rst_port: rst.map(str::to_owned),
                ports,
                role: String::new(),
                origin_info: String::new(),
            },
            valid_port: Some(valid.into()),
            ready_port: Some(ready.into()),
            data_ports: Vec::new(),
            extra: BTreeMap::default(),
        }
    };

    for def in module_defs {
        let name = def.name();
        let port_names: std::collections::HashSet<String> =
            def.ports().iter().map(|p| p.name.clone()).collect();

        let module_ifaces = if name == "fifo" {
            vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["clk".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()),
                        rst_port: None,
                        ports: vec!["clk".into(), "reset".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                make_hs(
                    vec![
                        "if_din".into(),
                        "if_full_n".into(),
                        "if_write".into(),
                        "clk".into(),
                        "reset".into(),
                    ],
                    Some("clk"),
                    Some("reset"),
                    "if_write",
                    "if_full_n",
                ),
                make_hs(
                    vec![
                        "if_dout".into(),
                        "if_empty_n".into(),
                        "if_read".into(),
                        "clk".into(),
                        "reset".into(),
                    ],
                    Some("clk"),
                    Some("reset"),
                    "if_empty_n",
                    "if_read",
                ),
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["if_read_ce".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["if_write_ce".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ]
        } else if name == "reset_inverter" {
            vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["clk".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()),
                        rst_port: None,
                        ports: vec!["clk".into(), "rst_n".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()),
                        rst_port: None,
                        ports: vec!["clk".into(), "rst".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ]
        } else if name.ends_with("_control_s_axi") {
            let mut ci = vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["ACLK".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("ACLK".into()),
                        rst_port: None,
                        ports: vec!["ACLK".into(), "ARESET".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None,
                        rst_port: None,
                        ports: vec!["ACLK_EN".into()],
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ];
            // 5 AXI-Lite channel handshakes
            for ch in tapa_protocol::S_AXI_LITE_CHANNELS {
                let (valid, ready) = (ch.valid, ch.ready);
                ci.push(make_hs(
                    ch.ports.iter().map(|&s| s.to_owned()).collect(),
                    Some("ACLK"),
                    Some("ARESET"),
                    valid,
                    ready,
                ));
            }
            ci.push(AnyInterface::FeedForward {
                base: InterfaceBase {
                    clk_port: Some("ACLK".into()),
                    rst_port: Some("ARESET".into()),
                    ports: vec!["ACLK".into(), "ARESET".into(), "interrupt".into()],
                    role: String::new(),
                    origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            // ApCtrl with scalar ports + control
            let mut ap_ports: Vec<String> = def
                .ports()
                .iter()
                .map(|p| p.name.clone())
                .filter(|n| !ctrl_s_axi_fixed_ports().contains(n.as_str()))
                .collect();
            ap_ports.extend(
                [
                    "ACLK",
                    "ARESET",
                    HANDSHAKE_START,
                    HANDSHAKE_DONE,
                    HANDSHAKE_READY,
                    HANDSHAKE_IDLE,
                ]
                .iter()
                .map(|&s| s.to_owned()),
            );
            ci.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some("ACLK".into()),
                    rst_port: Some("ARESET".into()),
                    ports: ap_ports,
                    role: String::new(),
                    origin_info: String::new(),
                },
                ap_start_port: Some(HANDSHAKE_START.into()),
                ap_done_port: Some(HANDSHAKE_DONE.into()),
                ap_ready_port: Some(HANDSHAKE_READY.into()),
                ap_idle_port: Some(HANDSHAKE_IDLE.into()),
                ap_continue_port: None,
                extra: BTreeMap::default(),
            });
            ci
        } else if name == program.top {
            // Top task: stream/MMAP interfaces + optional s_axi_control
            // channel handshakes (only if the top module actually exposes
            // the `s_axi_control_*` ports — i.e. ctrl_s_axi is present).
            let mut ti = build_task_port_ifaces(def, &port_names);
            let has_s_axi_ports = port_names.contains(format!("{S_AXI_NAME}_ARVALID").as_str());
            if has_s_axi_ports {
                for ch in tapa_protocol::S_AXI_LITE_CHANNELS {
                    let (valid, ready) = (ch.valid, ch.ready);
                    let mut ports: Vec<String> = ch
                        .ports
                        .iter()
                        .map(|&s| format!("{S_AXI_NAME}_{s}"))
                        .collect();
                    ports.extend([HANDSHAKE_CLK.into(), HANDSHAKE_RST_N.into()]);
                    ti.push(make_hs(
                        ports,
                        Some(HANDSHAKE_CLK),
                        Some(HANDSHAKE_RST_N),
                        &format!("{S_AXI_NAME}_{valid}"),
                        &format!("{S_AXI_NAME}_{ready}"),
                    ));
                }
            }
            ti
        } else if slot_to_instances.contains_key(name) {
            // Slot task: stream/MMAP interfaces + ApCtrl + Clock + FeedForwardReset
            let mut si = Vec::new();
            let mut scalars = Vec::new();
            build_task_port_ifaces_with_scalars(def, &port_names, &mut si, &mut scalars);
            let mut ap_ports = scalars;
            ap_ports.extend(
                [
                    HANDSHAKE_CLK,
                    HANDSHAKE_RST_N,
                    HANDSHAKE_START,
                    HANDSHAKE_DONE,
                    HANDSHAKE_READY,
                    HANDSHAKE_IDLE,
                ]
                .iter()
                .map(|&s| s.to_owned()),
            );
            si.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some(HANDSHAKE_CLK.into()),
                    rst_port: Some(HANDSHAKE_RST_N.into()),
                    ports: ap_ports,
                    role: String::new(),
                    origin_info: String::new(),
                },
                ap_start_port: Some(HANDSHAKE_START.into()),
                ap_done_port: Some(HANDSHAKE_DONE.into()),
                ap_ready_port: Some(HANDSHAKE_READY.into()),
                ap_idle_port: Some(HANDSHAKE_IDLE.into()),
                ap_continue_port: None,
                extra: BTreeMap::default(),
            });
            si.push(AnyInterface::Clock {
                base: InterfaceBase {
                    clk_port: None,
                    rst_port: None,
                    ports: vec![HANDSHAKE_CLK.into()],
                    role: String::new(),
                    origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            si.push(AnyInterface::FeedForwardReset {
                base: InterfaceBase {
                    clk_port: Some(HANDSHAKE_CLK.into()),
                    rst_port: None,
                    ports: vec![HANDSHAKE_CLK.into(), HANDSHAKE_RST_N.into()],
                    role: String::new(),
                    origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            si
        } else if name.ends_with("_fsm") && name == format!("{}_fsm", program.top) {
            // Top-level FSM: emit per-slot and top-level ApCtrl interfaces.
            let mut fi = Vec::new();
            let slot_names: Vec<String> = slot_to_instances.keys().cloned().collect();

            for slot_name in &slot_names {
                let slot_prefix = format!("{slot_name}_0");
                let start = format!("{slot_prefix}__{HANDSHAKE_START}");
                let done = format!("{slot_prefix}__{HANDSHAKE_DONE}");
                let ready = format!("{slot_prefix}__{HANDSHAKE_READY}");
                let idle = format!("{slot_prefix}__{HANDSHAKE_IDLE}");
                // Minimal fixtures may omit slot-prefixed handshake ports.
                if !port_names.contains(&start) || !port_names.contains(&done) {
                    continue;
                }
                let mut ap_ports: Vec<String> = vec![HANDSHAKE_CLK.into(), HANDSHAKE_RST_N.into()];
                ap_ports.extend(
                    def.ports()
                        .iter()
                        .filter(|p| p.name.starts_with(&slot_prefix))
                        .map(|p| p.name.clone()),
                );
                fi.push(AnyInterface::ApCtrl {
                    base: InterfaceBase {
                        clk_port: Some(HANDSHAKE_CLK.into()),
                        rst_port: Some(HANDSHAKE_RST_N.into()),
                        ports: ap_ports,
                        role: String::new(),
                        origin_info: String::new(),
                    },
                    ap_start_port: Some(start),
                    ap_done_port: Some(done),
                    ap_ready_port: Some(ready),
                    ap_idle_port: Some(idle),
                    ap_continue_port: None,
                    extra: BTreeMap::default(),
                });
            }

            // FSM-top ApCtrl: scalar ports (excluding clock/reset/per-slot-prefixed) + ap_*.
            // Matches `get_fsm_ifaces`, which emits this interface
            // unconditionally on the top FSM module. Role inference then
            // validates the directions.
            let fsm_scalars: Vec<String> = def
                .ports()
                .iter()
                .map(|p| p.name.clone())
                .filter(|pn| {
                    pn != HANDSHAKE_CLK
                        && pn != HANDSHAKE_RST_N
                        && !slot_names
                            .iter()
                            .any(|slot| pn.starts_with(&format!("{slot}_0")))
                })
                .collect();
            let mut top_ap_ports = fsm_scalars;
            top_ap_ports.extend(
                [
                    HANDSHAKE_CLK,
                    HANDSHAKE_RST_N,
                    HANDSHAKE_START,
                    HANDSHAKE_DONE,
                    HANDSHAKE_READY,
                    HANDSHAKE_IDLE,
                ]
                .iter()
                .map(|&s| s.to_owned()),
            );
            fi.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some(HANDSHAKE_CLK.into()),
                    rst_port: Some(HANDSHAKE_RST_N.into()),
                    ports: top_ap_ports,
                    role: String::new(),
                    origin_info: String::new(),
                },
                ap_start_port: Some(HANDSHAKE_START.into()),
                ap_done_port: Some(HANDSHAKE_DONE.into()),
                ap_ready_port: Some(HANDSHAKE_READY.into()),
                ap_idle_port: Some(HANDSHAKE_IDLE.into()),
                ap_continue_port: None,
                extra: BTreeMap::default(),
            });
            fi
        } else {
            // Other modules (leaf, non-top FSM): skip — interfaces are
            // established at the integration layer that owns this module.
            Vec::new()
        };

        if !module_ifaces.is_empty() {
            ifaces.insert(name.to_owned(), module_ifaces);
        }
    }
    ifaces
}

/// Build stream and MMAP handshake interfaces for a task module's ports.
pub fn build_task_port_ifaces(
    def: &AnyModuleDefinition,
    port_names: &std::collections::HashSet<String>,
) -> Vec<AnyInterface> {
    let mut ifaces = Vec::new();
    let mut unused_scalars = Vec::new();
    build_task_port_ifaces_with_scalars(def, port_names, &mut ifaces, &mut unused_scalars);
    ifaces
}

/// Build stream/MMAP interfaces and collect scalar port names.
///
/// Scalar ports (and MMAP `_offset` scalars) are appended to `scalars`;
/// stream and MMAP handshake interfaces are appended to `ifaces`.
#[allow(
    clippy::too_many_lines,
    reason = "port interface assembly is inherently sequential; \
              splitting would fragment the scalar/stream/mmap wiring"
)]
pub fn build_task_port_ifaces_with_scalars(
    def: &AnyModuleDefinition,
    port_names: &std::collections::HashSet<String>,
    ifaces: &mut Vec<AnyInterface>,
    scalars: &mut Vec<String>,
) {
    use tapa_graphir::interface::InterfaceBase;
    let ports = def.ports();
    let mut seen = std::collections::HashSet::new();

    for port in ports {
        // Skip system ports
        if port.name.starts_with("ap_") || port.name.starts_with("s_axi_control_") {
            continue;
        }

        // Detect stream triplets
        let is_istream = port.name.ends_with("_dout")
            || port.name.ends_with("_empty_n")
            || port.name.ends_with("_read");
        let is_output_stream = port.name.ends_with("_din")
            || port.name.ends_with("_full_n")
            || port.name.ends_with("_write");
        let is_mmap = port.name.starts_with("m_axi_") || port.name.ends_with("_offset");

        if is_mmap {
            // MMAP: handled separately below
            continue;
        }

        if is_istream || is_output_stream {
            let Some(base) = extract_stream_base(&port.name) else {
                continue;
            };
            if !seen.insert(format!("stream:{base}")) {
                continue;
            }
            // ostream: valid=_write, ready=_full_n; istream: valid=_empty_n, ready=_read.
            let is_out = port_names.contains(&format!("{base}_din"));
            let (suffixes, valid_suffix, ready_suffix): (&[&str], &str, &str) = if is_out {
                (OSTREAM_SUFFIXES, "_write", "_full_n")
            } else {
                (ISTREAM_SUFFIXES, "_empty_n", "_read")
            };
            let mut ps: Vec<String> = suffixes.iter().map(|s| format!("{base}{s}")).collect();
            ps.extend([HANDSHAKE_CLK.into(), HANDSHAKE_RST_N.into()]);
            ifaces.push(AnyInterface::HandShake {
                base: InterfaceBase {
                    clk_port: Some(HANDSHAKE_CLK.into()),
                    rst_port: Some(HANDSHAKE_RST_N.into()),
                    ports: ps,
                    role: String::new(),
                    origin_info: String::new(),
                },
                valid_port: Some(format!("{base}{valid_suffix}")),
                ready_port: Some(format!("{base}{ready_suffix}")),
                data_ports: Vec::new(),
                extra: BTreeMap::default(),
            });
            continue;
        }

        // Scalar port
        scalars.push(port.name.clone());
    }

    // MMAP interfaces: group by arg name, per AXI channel
    let mut mmap_bases = std::collections::BTreeSet::new();
    for port in ports {
        if port.name.ends_with("_offset") && !port.name.starts_with(M_AXI_PREFIX) {
            let base = port.name.trim_end_matches("_offset");
            mmap_bases.insert(base.to_owned());
        }
    }
    for base in &mmap_bases {
        scalars.push(format!("{base}_offset"));
        // Per-channel MMAP handshakes
        for (channel_ports, valid_suffix, ready_suffix) in [
            (
                &[
                    "_ARVALID",
                    "_ARREADY",
                    "_ARADDR",
                    "_ARID",
                    "_ARLEN",
                    "_ARSIZE",
                    "_ARBURST",
                    "_ARLOCK",
                    "_ARCACHE",
                    "_ARPROT",
                    "_ARQOS",
                    "_ARREGION",
                ][..],
                "_ARVALID",
                "_ARREADY",
            ),
            (
                &["_RVALID", "_RREADY", "_RDATA", "_RLAST", "_RID", "_RRESP"][..],
                "_RVALID",
                "_RREADY",
            ),
            (
                &[
                    "_AWVALID",
                    "_AWREADY",
                    "_AWADDR",
                    "_AWID",
                    "_AWLEN",
                    "_AWSIZE",
                    "_AWBURST",
                    "_AWLOCK",
                    "_AWCACHE",
                    "_AWPROT",
                    "_AWQOS",
                    "_AWREGION",
                ][..],
                "_AWVALID",
                "_AWREADY",
            ),
            (
                &["_WVALID", "_WREADY", "_WDATA", "_WSTRB", "_WLAST"][..],
                "_WVALID",
                "_WREADY",
            ),
            (
                &["_BVALID", "_BREADY", "_BID", "_BRESP"][..],
                "_BVALID",
                "_BREADY",
            ),
        ] {
            let valid_port = format!("m_axi_{base}{valid_suffix}");
            let ready_port = format!("m_axi_{base}{ready_suffix}");
            if !port_names.contains(&valid_port) || !port_names.contains(&ready_port) {
                continue;
            }
            let mut ch_ports: Vec<String> = channel_ports
                .iter()
                .map(|s| format!("m_axi_{base}{s}"))
                .filter(|n| port_names.contains(n))
                .collect();
            ch_ports.extend([HANDSHAKE_CLK.into(), HANDSHAKE_RST_N.into()]);
            ifaces.push(AnyInterface::HandShake {
                base: InterfaceBase {
                    clk_port: Some(HANDSHAKE_CLK.into()),
                    rst_port: Some(HANDSHAKE_RST_N.into()),
                    ports: ch_ports,
                    role: String::new(),
                    origin_info: String::new(),
                },
                valid_port: Some(valid_port),
                ready_port: Some(ready_port),
                data_ports: Vec::new(),
                extra: BTreeMap::default(),
            });
        }
    }
}

/// Fixed (non-scalar) `ctrl_s_axi` ports: clocking/reset/interrupt,
/// the AXI-Lite channel ports, and the ap-ctrl handshakes.
fn ctrl_s_axi_fixed_ports() -> std::collections::BTreeSet<&'static str> {
    ["ACLK", "ACLK_EN", "ARESET", "interrupt"]
        .into_iter()
        .chain(S_AXI_LITE_CTRL_PORTS.iter().copied())
        .chain([
            HANDSHAKE_START,
            HANDSHAKE_DONE,
            HANDSHAKE_IDLE,
            HANDSHAKE_READY,
        ])
        .collect()
}

/// Extract stream base name from a suffixed port name.
fn extract_stream_base(name: &str) -> Option<&str> {
    for suffix in ISTREAM_SUFFIXES.iter().chain(OSTREAM_SUFFIXES) {
        if let Some(base) = name.strip_suffix(suffix) {
            return Some(base);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmap_interfaces_are_ordered_by_port_name() {
        let ports = vec![
            crate::utils::input_wire("z_offset", None),
            crate::utils::output_wire("m_axi_z_AWVALID", None),
            crate::utils::input_wire("m_axi_z_AWREADY", None),
            crate::utils::input_wire("a_offset", None),
            crate::utils::output_wire("m_axi_a_AWVALID", None),
            crate::utils::input_wire("m_axi_a_AWREADY", None),
        ];
        let def = AnyModuleDefinition::new_verilog("task".to_owned(), ports, String::new());
        let port_names = def.ports().iter().map(|p| p.name.clone()).collect();

        let ifaces = build_task_port_ifaces(&def, &port_names);
        let valid_ports: Vec<&str> = ifaces
            .iter()
            .map(|iface| {
                let AnyInterface::HandShake { valid_port, .. } = iface else {
                    panic!("expected only mmap handshakes, got {iface:?}");
                };
                valid_port.as_deref().expect("handshake valid port")
            })
            .collect();

        assert_eq!(valid_ports, ["m_axi_a_AWVALID", "m_axi_z_AWVALID"],);
    }
}
