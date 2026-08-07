//! M-AXI memory-mapped connection aggregation.
//!
//! [`MMapConnection`]/[`MMapSlave`] are the crossbar-facing view of a shared
//! mmap argument: child ports bound to one argument become one slave each,
//! with channel geometry validated and nested aggregation rolled up into
//! parent-facing ID widths. The [`direct`] submodule catalogs child M-AXI
//! interfaces wired straight to a top-level mmap port.

pub mod direct;

use std::collections::BTreeMap;

use tapa_ir::task::TaskLevel;
use tapa_ir::Port as IrPort;
use tapa_protocol::{axi_subport_from_suffix, M_AXI_SUFFIXES_COMPACT};
use tapa_rtl::expression::{expression_as_u32, expression_source, Expression};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::port::Port as RtlPort;
use tapa_rtl::VerilogModule;

use crate::error::CodegenError;
use crate::state::rtl_state::{routing_id_bits, TopologyWithRtl};

fn validate_mmap_channel_geometry(
    parent_task_name: &str,
    parent_port_name: &str,
    parent: &IrPort,
    child_task_name: &str,
    child_port_name: &str,
    child: &IrPort,
) -> Result<(), CodegenError> {
    match (parent.chan_count, child.chan_count) {
        (Some(parent_count), Some(child_count)) if parent_count != child_count => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap channel-count mismatch: '{parent_task_name}.{parent_port_name}' declares \
                 {parent_count}, but '{child_task_name}.{child_port_name}' declares {child_count}",
            )));
        }
        _ => {}
    }
    match (parent.chan_size, child.chan_size) {
        (Some(parent_size), Some(child_size)) if parent_size != child_size => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap channel-size mismatch: '{parent_task_name}.{parent_port_name}' declares \
                 {parent_size}, but '{child_task_name}.{child_port_name}' declares {child_size}",
            )));
        }
        _ => {}
    }
    Ok(())
}

fn merge_mmap_port_metadata(
    parent_task_name: &str,
    parent_port_name: &str,
    parent: Option<&IrPort>,
    child_task_name: &str,
    child_port_name: &str,
    child: Option<&IrPort>,
) -> Result<(u32, Option<u32>, Option<u32>), CodegenError> {
    if let (Some(parent), Some(child)) = (parent, child) {
        if parent.width != child.width {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap width mismatch: '{parent_task_name}.{parent_port_name}' is {} bits, but \
                 '{child_task_name}.{child_port_name}' is {} bits",
                parent.width, child.width,
            )));
        }
        validate_mmap_channel_geometry(
            parent_task_name,
            parent_port_name,
            parent,
            child_task_name,
            child_port_name,
            child,
        )?;
    }

    let data_width = parent.or(child).map(|port| port.width).ok_or_else(|| {
        CodegenError::InvalidMmapConnection(format!(
            "no port named '{parent_port_name}' on task '{parent_task_name}' or \
             '{child_port_name}' on child '{child_task_name}' to derive the mmap data width from",
        ))
    })?;
    let chan_count = parent
        .and_then(|port| port.chan_count)
        .or_else(|| child.and_then(|port| port.chan_count));
    let chan_size = parent
        .and_then(|port| port.chan_size)
        .or_else(|| child.and_then(|port| port.chan_size));
    Ok((data_width, chan_count, chan_size))
}

/// One crossbar slave: a child port bound to a shared mmap argument.
#[derive(Debug, Clone)]
pub struct MMapSlave {
    /// Child task name.
    pub task: String,
    /// Instance index within the child task's instantiation list.
    pub inst_idx: u32,
    /// Child-side port name.
    pub port: String,
    /// Aggregated AXI thread count this port presents: a leaf child
    /// contributes 1; an upper child that internally shares the mmap
    /// contributes its own aggregated total, so the crossbar's
    /// `S*_THREADS` can track every outstanding ID.
    pub threads: u32,
    /// AXI ID width the child port presents (from its RTL module and
    /// any nested aggregation).
    pub id_width: u32,
}

/// Aggregated M-AXI memory-mapped connection info for a single argument.
#[derive(Debug, Clone)]
pub struct MMapConnection {
    /// Argument name.
    pub arg_name: String,
    /// The child ports sharing this argument, one crossbar slave each.
    pub slaves: Vec<MMapSlave>,
    /// Channel count; `None` for a plain (non-hmap) mmap. `Some(1)`
    /// is a single-channel hmap, which still gets a crossbar.
    pub chan_count: Option<u32>,
    /// Channel size in elements; `None` for a plain mmap.
    pub chan_size: Option<u32>,
    /// Data width in bits.
    pub data_width: u32,
}

impl MMapConnection {
    /// Parent-facing AXI ID width: the widest child ID plus enough
    /// routing bits to identify the originating slave.
    #[must_use]
    pub fn id_width(&self) -> u32 {
        let widest_child = self.slaves.iter().map(|s| s.id_width).max().unwrap_or(1);
        id_width_for_child_threads(widest_child, self.thread_count())
    }

    /// Crossbar slave count.
    #[must_use]
    pub fn thread_count(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation, reason = "slave count fits in u32")]
        {
            self.slaves.len() as u32
        }
    }

    /// Upstream channel count; a plain mmap behaves as one channel.
    #[must_use]
    pub fn channel_count(&self) -> u32 {
        self.chan_count.unwrap_or(1)
    }

    /// Sum of the per-slave aggregated thread counts.
    #[must_use]
    pub fn total_threads(&self) -> u32 {
        self.slaves.iter().map(|s| s.threads).sum()
    }
}

impl TopologyWithRtl {
    /// Aggregate M-AXI `MMapConnection` data from topology instances.
    ///
    /// For each upper-level task, collects all mmap/`async_mmap` arguments
    /// from child instances and groups them by argument name.
    pub fn aggregate_mmap_connections(
        &self,
        task_name: &str,
    ) -> Result<BTreeMap<String, MMapConnection>, CodegenError> {
        self.aggregate_mmap_connections_cached(task_name, &mut BTreeMap::new())
    }

    /// Recursive worker for [`Self::aggregate_mmap_connections`]. The
    /// cache holds each upper task's finished aggregation so nested
    /// hierarchies are aggregated once per task, not once per lookup.
    fn aggregate_mmap_connections_cached(
        &self,
        task_name: &str,
        cache: &mut BTreeMap<String, BTreeMap<String, MMapConnection>>,
    ) -> Result<BTreeMap<String, MMapConnection>, CodegenError> {
        let task = self
            .design
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;

        let mut connections: BTreeMap<String, MMapConnection> = BTreeMap::new();

        for (child_task_name, instances) in &task.tasks {
            for (inst_idx, instance) in instances.iter().enumerate() {
                for (child_port_name, arg) in &instance.args {
                    let is_mmap = arg.cat.is_direct_mmap();
                    if !is_mmap {
                        continue;
                    }

                    // Look up child port metadata using the child's port name
                    let child_task = self.design.tasks.get(child_task_name.as_str());
                    let port = child_task
                        .and_then(|t| t.ports.iter().find(|p| p.name == *child_port_name));
                    let (child_id_width, child_threads) =
                        self.child_mmap_summary(child_task_name, child_port_name, cache)?;

                    // Group by parent scope arg name, not child port name. An
                    // mmap never binds a constant, so an unnamed one has no
                    // connection to record.
                    let Some(parent_arg_name) = arg.name() else {
                        continue;
                    };
                    let parent_port = task.ports.iter().find(|p| p.name == *parent_arg_name);

                    let (data_width, chan_count, chan_size) = merge_mmap_port_metadata(
                        task_name,
                        parent_arg_name,
                        parent_port,
                        child_task_name,
                        child_port_name,
                        port,
                    )?;
                    let conn = connections
                        .entry(parent_arg_name.to_owned())
                        .or_insert_with(|| MMapConnection {
                            arg_name: parent_arg_name.to_owned(),
                            slaves: Vec::new(),
                            chan_count,
                            chan_size,
                            data_width,
                        });
                    if conn.data_width != data_width {
                        return Err(CodegenError::InvalidMmapConnection(format!(
                            "mmap argument '{task_name}.{parent_arg_name}' has conflicting data \
                             widths: {} vs {data_width} at '{child_task_name}.{child_port_name}'",
                            conn.data_width
                        )));
                    }
                    if conn.chan_count != chan_count || conn.chan_size != chan_size {
                        return Err(CodegenError::InvalidMmapConnection(format!(
                            "mmap argument '{task_name}.{parent_arg_name}' has conflicting \
                             channel shapes: ({:?}, {:?}) vs ({chan_count:?}, {chan_size:?}) \
                             at '{child_task_name}.{child_port_name}'",
                            conn.chan_count, conn.chan_size
                        )));
                    }
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "instance index fits in u32"
                    )]
                    let idx = inst_idx as u32;
                    conn.slaves.push(MMapSlave {
                        task: child_task_name.clone(),
                        inst_idx: idx,
                        port: child_port_name.clone(),
                        threads: child_threads,
                        id_width: child_id_width,
                    });
                }
            }
        }

        // An hmap whose single child port internally shares the mmap
        // would need per-channel routing of an already-multiplexed ID
        // stream, which the crossbar wrapper does not implement.
        for conn in connections.values() {
            let total_threads = conn.total_threads();
            if let [slave] = conn.slaves.as_slice() {
                if conn.chan_count.is_some() && total_threads > 1 {
                    return Err(CodegenError::InvalidMmapConnection(format!(
                        "hmap argument '{}' is driven only by '{}.{}', which \
                         internally shares the mmap ({total_threads} threads); \
                         this combination is not supported",
                        conn.arg_name, slave.task, slave.port
                    )));
                }
            }
        }

        Ok(connections)
    }

    /// The `(id_width, threads)` a child port presents to its parent:
    /// the RTL module's AXI ID port width combined with the child's own
    /// nested aggregation (an upper child sharing the mmap internally
    /// presents its aggregated totals; a leaf presents 1 thread).
    fn child_mmap_summary(
        &self,
        child_task_name: &str,
        child_port_name: &str,
        cache: &mut BTreeMap<String, BTreeMap<String, MMapConnection>>,
    ) -> Result<(u32, u32), CodegenError> {
        let child_port = sanitize_array_name(child_port_name);
        let prefix = format!("m_axi_{child_port}");
        // Planning policy: fold the ID-port widths with `max` for a safe
        // upper estimate (ID wires that are too narrow would corrupt
        // routing; wider ones cost a few bits). Absent modules and symbolic
        // widths fall back to the single-bit minimum.
        let module_id_width = self
            .module_map
            .get(child_task_name)
            .and_then(|module| {
                rtl_m_axi_id_widths(&module.inner, &prefix)
                    .into_iter()
                    .filter_map(|(_, _, width)| width)
                    .max()
            })
            .unwrap_or(1);

        let is_upper = self
            .design
            .tasks
            .get(child_task_name)
            .is_some_and(|task| task.level == TaskLevel::Upper);
        let (nested_id_width, threads) = if is_upper {
            if !cache.contains_key(child_task_name) {
                let conns = self.aggregate_mmap_connections_cached(child_task_name, cache)?;
                cache.insert(child_task_name.to_owned(), conns);
            }
            cache[child_task_name]
                .get(child_port_name)
                .map_or((1, 1), |conn| (conn.id_width(), conn.total_threads()))
        } else {
            (1, 1)
        };
        Ok((module_id_width.max(nested_id_width), threads))
    }
}

/// Compute parent-facing AXI ID width: 1 + ceil(log2(n)), minimum 1.
fn id_width_for_child_threads(child_id_width: u32, n: u32) -> u32 {
    child_id_width.max(1) + routing_id_bits(n)
}

/// The ID ports declared on `module` under the compact M-AXI `prefix`, each
/// paired with its resolved bit width (`None` when the width expression is
/// symbolic beyond a parameter default).
///
/// The folding policy is deliberately left to the caller — connection
/// *planning* (`child_mmap_summary`) takes the `max` for a safe upper
/// estimate, while direct-interface *validation* (`direct` submodule)
/// requires every listed width to resolve to one identical value. What must
/// not diverge between callers is *which* ports count as ID ports and how
/// widths are resolved, so both go through this helper.
fn rtl_m_axi_id_widths<'m>(
    module: &'m VerilogModule,
    prefix: &str,
) -> Vec<(&'static str, &'m RtlPort, Option<u32>)> {
    M_AXI_SUFFIXES_COMPACT
        .iter()
        .filter(|&&suffix| axi_subport_from_suffix(suffix) == "ID")
        .filter_map(|&suffix| {
            let port = module.find_port(&format!("{prefix}{suffix}"))?;
            Some((suffix, port, resolve_rtl_port_width(module, port)))
        })
        .collect()
}

fn resolve_rtl_port_width(module: &VerilogModule, port: &RtlPort) -> Option<u32> {
    let Some(width) = &port.width else {
        return Some(1);
    };
    let msb = resolve_width_endpoint(module, &width.msb)?;
    let lsb = resolve_width_endpoint(module, &width.lsb)?;
    msb.abs_diff(lsb).checked_add(1)
}

fn resolve_width_endpoint(module: &VerilogModule, expression: &Expression) -> Option<u32> {
    if let Some(value) = expression_as_u32(expression) {
        return Some(value);
    }
    let source = expression_source(expression).replace(' ', "");
    source.strip_suffix("-1").map_or_else(
        || resolve_parameter_default(module, &source),
        |parameter| resolve_parameter_default(module, parameter)?.checked_sub(1),
    )
}

fn resolve_parameter_default(module: &VerilogModule, name: &str) -> Option<u32> {
    let parameter = module
        .parameters
        .iter()
        .find(|parameter| parameter.name == name)?;
    expression_as_u32(&parameter.default)
}

#[cfg(test)]
mod tests {
    use tapa_ir::Design;
    use tapa_rtl::VerilogModule;

    use super::*;

    /// The shared mmap fixture port: `float*`/32-bit with no channel
    /// geometry. Tests override the 1-2 fields that make a parent-child
    /// connection invalid.
    fn mmap_port(name: &str) -> serde_json::Value {
        serde_json::json!({"cat": "mmap", "name": name, "type": "float*", "width": 32})
    }

    /// The shared reject-fixture design: `top` owns `parent_ports` and
    /// forwards its `elems` mmap argument to one leaf child per
    /// `(name, port)` entry, each bound under its own port name.
    fn mmap_design(
        parent_ports: &[serde_json::Value],
        children: &[(&str, serde_json::Value)],
    ) -> Design {
        let bindings: serde_json::Map<String, serde_json::Value> = children
            .iter()
            .map(|(name, port)| {
                (
                    (*name).to_string(),
                    serde_json::json!([{
                        "args": {port["name"].as_str().unwrap(): {"arg": "elems", "cat": "mmap"}}
                    }]),
                )
            })
            .collect();
        let mut tasks = serde_json::json!({
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": parent_ports,
                "tasks": bindings,
                "fifos": {}
            }
        });
        for (name, port) in children {
            tasks[*name] = serde_json::json!({
                "readable_name": name,
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [port],
                "tasks": {},
                "fifos": {}
            });
        }
        crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": tasks,
        }))
    }

    #[test]
    fn id_width_calculation() {
        assert_eq!(id_width_for_child_threads(1, 0), 1);
        assert_eq!(id_width_for_child_threads(1, 1), 1);
        assert_eq!(id_width_for_child_threads(1, 2), 2);
        assert_eq!(id_width_for_child_threads(1, 3), 3);
        assert_eq!(id_width_for_child_threads(1, 4), 3);
        assert_eq!(id_width_for_child_threads(1, 7), 4);
        assert_eq!(id_width_for_child_threads(1, 8), 4);
    }

    #[test]
    fn aggregate_mmap_connections_preserves_child_axi_id_width() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let mut state = TopologyWithRtl::new(program);
        state
            .attach_module(
                "mid",
                VerilogModule::parse(
                    "module mid(\n\
                     output wire [1:0] m_axi_data_ARID,\n\
                     output wire [1:0] m_axi_data_AWID,\n\
                     input wire [1:0] m_axi_data_BID,\n\
                     input wire [1:0] m_axi_data_RID\n\
                     ); endmodule",
                )
                .unwrap(),
            )
            .unwrap();

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            2,
            "parent-facing ID width must be at least as wide as the child AXI port"
        );
    }

    #[test]
    fn aggregate_propagates_nested_slave_threads() {
        // top shares `elems` between a leaf child and an upper child
        // (`mid`) that internally shares its `data` port between two
        // leaves. The parent-facing connection must report the
        // aggregated per-slave thread counts, not a flat 1.
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [{"args": {"d": {"arg": "elems", "cat": "mmap"}}}],
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let mid_conns = state.aggregate_mmap_connections("mid").unwrap();
        let mid_threads: Vec<u32> = mid_conns["data"].slaves.iter().map(|s| s.threads).collect();
        assert_eq!(mid_threads, vec![1, 1]);

        let top_conns = state.aggregate_mmap_connections("top").unwrap();
        let conn = &top_conns["elems"];
        assert_eq!(conn.thread_count(), 2, "two slave ports at top level");
        // Task iteration is alphabetical: `leaf` (1 thread) then `mid`
        // (2 aggregated threads).
        let threads: Vec<u32> = conn.slaves.iter().map(|s| s.threads).collect();
        assert_eq!(threads, vec![1, 2]);
    }

    #[test]
    fn aggregate_rejects_hmap_with_internally_shared_child() {
        // A single hmap child port whose task internally shares the
        // mmap: unsupported, rejected at aggregation time.
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32,
                         "chan_count": 2, "chan_size": 1024}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);
        let result = state.aggregate_mmap_connections("top");
        assert!(
            matches!(result, Err(CodegenError::InvalidMmapConnection(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn aggregate_rejects_conflicting_channel_shapes() {
        // One consumer declares channel geometry on its `d` port while
        // the other is plain: the shapes must agree across consumers.
        let mut chan_port = mmap_port("d");
        chan_port["chan_count"] = serde_json::json!(2);
        chan_port["chan_size"] = serde_json::json!(1024);
        let state = TopologyWithRtl::new(mmap_design(
            &[],
            &[("chan_leaf", chan_port), ("plain_leaf", mmap_port("d"))],
        ));
        let result = state.aggregate_mmap_connections("top");
        assert!(
            matches!(result, Err(CodegenError::InvalidMmapConnection(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn aggregate_rejects_parent_child_data_width_mismatch() {
        let mut parent = mmap_port("elems");
        parent["type"] = serde_json::json!("long*");
        parent["width"] = serde_json::json!(64);
        let mut child = mmap_port("data");
        child["type"] = serde_json::json!("int*");
        let state = TopologyWithRtl::new(mmap_design(&[parent], &[("leaf", child)]));

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched AXI widths must be rejected");
        assert!(err.to_string().contains("64 bits"), "got: {err}");
        assert!(err.to_string().contains("32 bits"), "got: {err}");
    }

    #[test]
    fn aggregate_rejects_parent_child_channel_count_mismatch() {
        let mut parent = mmap_port("elems");
        parent["chan_count"] = serde_json::json!(2);
        parent["chan_size"] = serde_json::json!(1024);
        let mut child = mmap_port("data");
        child["chan_count"] = serde_json::json!(4);
        child["chan_size"] = serde_json::json!(1024);
        let state = TopologyWithRtl::new(mmap_design(&[parent], &[("leaf", child)]));

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched channel counts must be rejected");
        assert!(err.to_string().contains("channel-count mismatch"));
        assert!(err.to_string().contains("top.elems' declares 2"));
        assert!(err.to_string().contains("leaf.data' declares 4"));
    }

    #[test]
    fn aggregate_rejects_parent_child_channel_size_mismatch() {
        let mut parent = mmap_port("elems");
        parent["chan_count"] = serde_json::json!(2);
        parent["chan_size"] = serde_json::json!(1024);
        let mut child = mmap_port("data");
        child["chan_count"] = serde_json::json!(2);
        child["chan_size"] = serde_json::json!(2048);
        let state = TopologyWithRtl::new(mmap_design(&[parent], &[("leaf", child)]));

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched channel sizes must be rejected");
        assert!(err.to_string().contains("channel-size mismatch"));
        assert!(err.to_string().contains("top.elems' declares 1024"));
        assert!(err.to_string().contains("leaf.data' declares 2048"));
    }

    #[test]
    fn aggregate_mmap_connections_preserves_nested_child_crossbar_id_width() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            2,
            "parent-facing ID width should preserve nested child crossbar routing bits"
        );
    }

    #[test]
    fn aggregate_mmap_connections_expands_wide_child_id_for_parent_crossbar() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}],
                        "leaf": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            3,
            "parent crossbar should append routing bits to the widest child AXI ID"
        );
    }
}
