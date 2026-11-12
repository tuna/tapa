//! Cosim port metadata for ZIP packages.
//!
//! The HLS and SLASH ZIPs embed `tapa.json`, which `frt-cosim` projects
//! into the simulator's argument list.
//! [`stamp_cosim_port_metadata`] fills the optional per-port cosim
//! metadata (`stream_depth` / `mmap_addr_width` on
//! [`tapa_ir::port::Port`]) on the archive's embedded state copy, so the
//! runtime reads the values out of the schema instead of assuming them.

use tapa_ir::WorkState;

/// FIFO depth stamped onto every top-level stream port in the archive.
///
/// No per-port depth exists upstream: `tapacc` deliberately omits
/// `depth` for external top-level FIFOs (kernel-boundary streams), and
/// codegen wires them straight through without storage — a top-level
/// stream port has no FIFO on the RTL side at all, so the depth is a
/// pure cosimulation-buffer fact.
const DEFAULT_STREAM_DEPTH: u32 = 16;

/// AXI address width stamped onto every top-level direct mmap port.
///
/// There is no per-port choice today: every direct `m_axi` interface
/// TAPA generates is 64-bit — `tapa-codegen` builds the Vitis bridge
/// wiring and validates the resulting RTL port widths against its
/// (uniform) 64-bit address width, the AXI interconnect masters are
/// instantiated with `ADDR_WIDTH = 64`, and Vitis HLS defaults `m_axi`
/// addresses to 64 bits. Stamping that uniform value lets `frt-cosim`
/// size its model from the archive instead of assuming 64 itself.
const DEFAULT_MMAP_ADDR_WIDTH: u32 = 64;

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
pub(super) fn stamp_cosim_port_metadata(state: &WorkState) -> WorkState {
    let mut state = state.clone();
    if let Some(top_task) = state.graph.tasks.get_mut(&state.graph.top) {
        for port in &mut top_task.ports {
            if port.cat.is_stream() {
                port.stream_depth.get_or_insert(DEFAULT_STREAM_DEPTH);
            } else if port.cat.is_mmap_like() {
                port.mmap_addr_width.get_or_insert(DEFAULT_MMAP_ADDR_WIDTH);
            }
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::ClockPeriod;

    use std::collections::BTreeMap;

    use tapa_ir::{
        port::{ArgCategory, Port},
        Target, Task, TaskGraph, TaskLevel,
    };

    use crate::testutil::{port, task};

    fn state_with_ports(ports: Vec<Port>) -> WorkState {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_owned(),
            Task {
                level: TaskLevel::Upper,
                ports,
                readable_name: "Top".to_owned(),
                clock_period: Some(ClockPeriod::from_picoseconds(3330)),
                ..task("Top")
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
            port("gmem", ArgCategory::Mmap, "int", 32),
            port("amap", ArgCategory::AsyncMmap, "int", 32),
            port("si", ArgCategory::Istream, "int", 32),
            port("so", ArgCategory::Ostreams, "int", 32),
            port("n", ArgCategory::Scalar, "int", 32),
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
        let mut s = port("si", ArgCategory::Istream, "int", 32);
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
        let mut state = state_with_ports(vec![port("gmem", ArgCategory::Mmap, "int", 32)]);
        state.graph.top = "Gone".to_owned();
        let stamped = stamp_cosim_port_metadata(&state);
        assert_eq!(stamped.graph.top, "Gone", "stamping is a no-op");
    }
}
