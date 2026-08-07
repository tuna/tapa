//! Cosim-compatibility frontier for the `xilinx-hls` zip archive.
//!
//! The zip `tapa pack` emits for the `xilinx-hls` target is the
//! hardware-cosimulation package: it embeds `tapa.json`, which
//! `frt-cosim` — a separate Cargo workspace under `fpga-runtime/` —
//! projects into the simulator's argument list. Two archive-level
//! contracts are therefore enforced here, at pack time, where a precise
//! error can still reach the user *before* the archive ships:
//!
//! * [`stamp_cosim_port_metadata`] fills the optional per-port cosim
//!   metadata (`stream_depth` / `mmap_addr_width` on
//!   [`tapa_ir::port::Port`]) on the archive's embedded state copy, so
//!   the runtime reads the values out of the schema instead of assuming
//!   them.
//! * [`check_cosim_supported_categories`] rejects top-level ports the
//!   cosim projection cannot bind.
//!
//! Two-workspace seam: the cosim-consumable category set is enforced
//! twice — here (pack time, the frontier) and in
//! `fpga-runtime/frt-cosim/src/metadata/zip_pkg.rs` (projection time,
//! the backstop for legacy or hand-built archives). Keep the two in
//! sync when the supported set changes; the `frt-cbindgen` drift guard
//! covers generated headers, not this seam.

use tapa_ir::{ArgCategory, WorkState};

use crate::error::{CliError, Result};

/// FIFO depth stamped onto every top-level stream port in the archive.
///
/// No per-port depth exists upstream: `tapacc` deliberately omits
/// `depth` for external top-level FIFOs (kernel-boundary streams), and
/// codegen wires them straight through without storage — a top-level
/// stream port has no FIFO on the RTL side at all, so the depth is a
/// pure cosimulation-buffer fact. 16 is the value `frt-cosim` has
/// always assumed (and still applies for pre-field archives), so
/// stamping it makes the archive say out loud what the runtime would
/// have assumed anyway.
pub const DEFAULT_STREAM_DEPTH: u32 = 16;

/// AXI address width stamped onto every top-level direct mmap port.
///
/// There is no per-port choice today: every direct `m_axi` interface
/// TAPA generates is 64-bit — `tapa-codegen` builds the Vitis bridge
/// wiring and validates the resulting RTL port widths against its
/// (uniform) 64-bit address width, the AXI interconnect masters are
/// instantiated with `ADDR_WIDTH = 64`, and Vitis HLS defaults `m_axi`
/// addresses to 64 bits. Stamping that uniform value lets `frt-cosim`
/// size its model from the archive instead of assuming 64 itself.
pub const DEFAULT_MMAP_ADDR_WIDTH: u32 = 64;

/// Reject top-level ports the cosim runtime cannot bind, before any
/// archive byte (or custom-RTL overlay) is written.
///
/// `immap` / `ommap` are read-only / write-only mmaps; the `xilinx-hls`
/// cosim flow has never wired them up. They remain supported on the
/// `xilinx-vitis` (bitstream) path, which projects them as `m_axi`
/// ports — so the error names the offending port(s), their category,
/// and both remediations rather than just failing late inside the
/// simulator projection.
pub fn check_cosim_supported_categories(state: &WorkState) -> Result<()> {
    let Some(top_task) = state.graph.tasks.get(&state.graph.top) else {
        // A missing top task stays the archive reader's error to report
        // (`top task '<top>' missing from tasks`); this frontier only
        // owns the category check.
        return Ok(());
    };
    let unsupported: Vec<String> = top_task
        .ports
        .iter()
        .filter(|port| matches!(port.cat, ArgCategory::Immap | ArgCategory::Ommap))
        .map(|port| format!("'{}' ({})", port.name, port.cat.as_str()))
        .collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(CliError::InvalidArg(format!(
        "unsupported port category for top-level port {}: the xilinx-hls \
         (hardware cosimulation) flow binds only scalar, stream, mmap, and \
         async_mmap arguments; change {} to `tapa::mmap` / \
         `tapa::async_mmap`, or package with the xilinx-vitis target, \
         where read-only / write-only mmaps are supported",
        unsupported.join(", "),
        if unsupported.len() == 1 { "it" } else { "them" },
    )))
}

/// Return `state` with the cosim port metadata stamped on the top
/// task's applicable ports, for the archive's embedded `tapa.json`
/// copy.
///
/// The work-dir state file is deliberately *not* mutated: the stamp is
/// a pure function of the persisted state, re-derived on every pack, so
/// the archive copy can never drift from the work dir's.
///
/// `get_or_insert`, not assignment: a port that already carries a value
/// from some producer keeps it — pack fills a schema gap, it does not
/// override.
#[must_use]
pub fn stamp_cosim_port_metadata(state: &WorkState) -> WorkState {
    let mut state = state.clone();
    if let Some(top_task) = state.graph.tasks.get_mut(&state.graph.top) {
        for port in &mut top_task.ports {
            if port.cat.is_stream() {
                port.stream_depth.get_or_insert(DEFAULT_STREAM_DEPTH);
            } else if port.cat.is_direct_mmap() {
                port.mmap_addr_width.get_or_insert(DEFAULT_MMAP_ADDR_WIDTH);
            }
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{
        port::{ArgCategory, Port},
        SynthTarget, Target, Task, TaskGraph, TaskLevel,
    };

    fn port(name: &str, cat: ArgCategory) -> Port {
        Port {
            cat,
            name: name.to_owned(),
            ctype: "int".to_owned(),
            width: 32,
            chan_count: None,
            chan_size: None,
            stream_depth: None,
            mmap_addr_width: None,
        }
    }

    fn state_with_ports(ports: Vec<Port>) -> WorkState {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_owned(),
            Task {
                level: TaskLevel::Upper,
                code: "void Top() {}".to_owned(),
                ports,
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: "Top".to_owned(),
                synth: SynthTarget::Hls,
                self_area: None,
                total_area: None,
                clock_period: "3.33".to_owned(),
            },
        );
        WorkState::new(TaskGraph {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_owned(),
            target: Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        })
    }

    fn stamped_ports(state: &WorkState) -> &[Port] {
        &state.graph.tasks["Top"].ports
    }

    #[test]
    fn stamp_fills_stream_depth_and_mmap_addr_width_where_applicable() {
        let state = state_with_ports(vec![
            port("gmem", ArgCategory::Mmap),
            port("amap", ArgCategory::AsyncMmap),
            port("si", ArgCategory::Istream),
            port("so", ArgCategory::Ostreams),
            port("n", ArgCategory::Scalar),
            port("ro", ArgCategory::Immap),
            port("wo", ArgCategory::Ommap),
        ]);
        let stamped = stamp_cosim_port_metadata(&state);
        let ports = stamped_ports(&stamped);
        assert_eq!(ports[0].mmap_addr_width, Some(64), "mmap stamped");
        assert_eq!(ports[1].mmap_addr_width, Some(64), "async_mmap stamped");
        assert_eq!(ports[2].stream_depth, Some(16), "istream stamped");
        assert_eq!(ports[3].stream_depth, Some(16), "ostreams stamped");
        assert_eq!(
            (ports[4].stream_depth, ports[4].mmap_addr_width),
            (None, None),
            "scalar carries neither field",
        );
        assert_eq!(
            (ports[5].mmap_addr_width, ports[6].mmap_addr_width),
            (None, None),
            "immap/ommap are not direct mmaps and are not stamped",
        );
        assert_eq!(
            (ports[5].stream_depth, ports[6].stream_depth),
            (None, None),
            "immap/ommap are not streams either",
        );
        for port in ports {
            if port.cat.is_stream() {
                assert_eq!(
                    port.mmap_addr_width, None,
                    "stream ports never grow an addr width: {}",
                    port.name,
                );
            }
        }
        // The input state is untouched (stamping works on a copy).
        assert_eq!(stamped_ports(&state)[0].mmap_addr_width, None);
    }

    #[test]
    fn stamp_preserves_values_a_producer_already_set() {
        let mut s = port("si", ArgCategory::Istream);
        s.stream_depth = Some(8);
        let stamped = stamp_cosim_port_metadata(&state_with_ports(vec![s]));
        assert_eq!(
            stamped_ports(&stamped)[0].stream_depth,
            Some(8),
            "an explicit producer value wins over the documented default",
        );
    }

    #[test]
    fn stamp_tolerates_a_missing_top_task() {
        let mut state = state_with_ports(vec![port("gmem", ArgCategory::Mmap)]);
        state.graph.top = "Gone".to_owned();
        let stamped = stamp_cosim_port_metadata(&state);
        assert_eq!(stamped.graph.top, "Gone", "stamping is a no-op");
    }

    #[test]
    fn frontier_accepts_every_supported_category() {
        let state = state_with_ports(vec![
            port("gmem", ArgCategory::Mmap),
            port("amap", ArgCategory::AsyncMmap),
            port("si", ArgCategory::Istream),
            port("so", ArgCategory::Ostreams),
            port("qi", ArgCategory::Istreams),
            port("qo", ArgCategory::Ostream),
            port("n", ArgCategory::Scalar),
        ]);
        check_cosim_supported_categories(&state).expect("all supported");
    }

    #[test]
    fn frontier_rejects_immap_and_ommap_with_names_and_remediation() {
        let state = state_with_ports(vec![
            port("gmem", ArgCategory::Mmap),
            port("ro_bank", ArgCategory::Immap),
            port("wo_bank", ArgCategory::Ommap),
        ]);
        let err = check_cosim_supported_categories(&state)
            .expect_err("immap/ommap must be rejected before the archive ships");
        let text = err.to_string();
        for needle in [
            "unsupported port category",
            "ro_bank",
            "wo_bank",
            "immap",
            "ommap",
            "xilinx-hls",
            "xilinx-vitis",
            "async_mmap",
        ] {
            assert!(
                text.contains(needle),
                "error must mention {needle}; got {text}"
            );
        }
    }
}
