//! Enriched topology + RTL state for code generation.
//!
//! `TopologyWithRtl` wraps a `Program` with attached Verilog modules
//! parsed from HLS output, plus FSM modules created during codegen.

use std::collections::BTreeMap;

use tapa_rtl::expression::Expression;
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::MutableModule;
use tapa_rtl::port::Width;
use tapa_rtl::VerilogModule;
use tapa_task_graph::port::ArgCategory;
use tapa_task_graph::task::TaskLevel;
use tapa_topology::program::Program;

use crate::error::CodegenError;

/// Aggregated M-AXI memory-mapped connection info for a single argument.
#[derive(Debug, Clone)]
pub struct MMapConnection {
    /// Argument name.
    pub arg_name: String,
    /// AXI ID width (log2 of total ports + 1).
    pub id_width: u32,
    /// Number of child instances using this mmap.
    pub thread_count: u32,
    /// Per-instance argument bindings: (`task_name`, `instance_idx`, `port_name`).
    pub args: Vec<(String, u32, String)>,
    /// Channel count (for hierarchical memory ports).
    pub chan_count: u32,
    /// Channel size.
    pub chan_size: u32,
    /// Data width in bits.
    pub data_width: u32,
}

/// Enriched state combining topology with RTL modules.
pub struct TopologyWithRtl {
    /// The topology model.
    pub program: Program,
    /// Parsed HLS Verilog modules, keyed by task name.
    pub module_map: BTreeMap<String, MutableModule>,
    /// FSM modules for upper-level tasks, keyed by task name.
    pub fsm_modules: BTreeMap<String, MutableModule>,
    /// Generated auxiliary RTL files, keyed by file path.
    pub generated_files: BTreeMap<String, String>,
}

impl TopologyWithRtl {
    /// Create a new `TopologyWithRtl` from a topology `Program`.
    pub fn new(program: Program) -> Self {
        Self {
            program,
            module_map: BTreeMap::new(),
            fsm_modules: BTreeMap::new(),
            generated_files: BTreeMap::new(),
        }
    }

    /// Attach a parsed HLS Verilog module to a task.
    ///
    /// Rejects nonexistent task names and duplicate attachments.
    pub fn attach_module(
        &mut self,
        task_name: &str,
        module: VerilogModule,
    ) -> Result<(), CodegenError> {
        if !self.program.tasks.contains_key(task_name) {
            return Err(CodegenError::TaskNotFound(task_name.to_owned()));
        }
        if self.module_map.contains_key(task_name) {
            return Err(CodegenError::ModuleAlreadyAttached(task_name.to_owned()));
        }
        self.module_map
            .insert(task_name.to_owned(), MutableModule::from_parsed(module));
        Ok(())
    }

    /// Attach multiple modules at once.
    pub fn attach_modules(
        &mut self,
        modules: BTreeMap<String, VerilogModule>,
    ) -> Result<(), CodegenError> {
        for (name, module) in modules {
            self.attach_module(&name, module)?;
        }
        Ok(())
    }

    /// Create an FSM module for an upper-level task.
    ///
    /// Rejects lower-level tasks.
    pub fn create_fsm_module(&mut self, task_name: &str) -> Result<(), CodegenError> {
        let task = self
            .program
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;

        if task.level == TaskLevel::Lower {
            return Err(CodegenError::FsmForLowerTask(task_name.to_owned()));
        }

        // Create an empty FSM module with the standard TAPA handshake ports.
        // The downstream lowering pass builds FSM interfaces (ApCtrl) that
        // reference ap_start / ap_done / ap_ready / ap_idle, so they must
        // be present on the FSM module definition.
        let fsm_name = format!("{task_name}_fsm");
        let fsm_source = format!(
            "module {fsm_name} (\n\
             input wire ap_clk,\n\
             input wire ap_rst_n,\n\
             input wire ap_start,\n\
             output wire ap_done,\n\
             output wire ap_ready,\n\
             output wire ap_idle\n\
             );\n\
             endmodule //{fsm_name}\n"
        );
        let parsed = VerilogModule::parse(&fsm_source)?;
        self.fsm_modules
            .insert(task_name.to_owned(), MutableModule::from_parsed(parsed));
        Ok(())
    }

    /// Aggregate M-AXI `MMapConnection` data from topology instances.
    ///
    /// For each upper-level task, collects all mmap/`async_mmap` arguments
    /// from child instances and groups them by argument name.
    pub fn aggregate_mmap_connections(
        &self,
        task_name: &str,
    ) -> Result<BTreeMap<String, MMapConnection>, CodegenError> {
        let task = self
            .program
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;

        let mut connections: BTreeMap<String, MMapConnection> = BTreeMap::new();

        for (child_task_name, instances) in &task.tasks {
            for (inst_idx, instance) in instances.iter().enumerate() {
                for (child_port_name, arg) in &instance.args {
                    let is_mmap = matches!(arg.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap);
                    if !is_mmap {
                        continue;
                    }

                    // Look up child port metadata using the child's port name
                    let child_task = self.program.tasks.get(child_task_name.as_str());
                    let port = child_task
                        .and_then(|t| t.ports.iter().find(|p| p.name == *child_port_name));
                    let child_id_width = self.child_mmap_id_width(child_task_name, child_port_name);

                    // Group by parent scope arg name (arg.arg), not child port name
                    let parent_arg_name = &arg.arg;
                    let parent_port = task.ports.iter().find(|p| p.name == *parent_arg_name);

                    let data_width = parent_port.or(port).map_or(64, |p| p.width);
                    let chan_count = parent_port
                        .and_then(|p| p.chan_count)
                        .or_else(|| port.and_then(|p| p.chan_count))
                        .unwrap_or(1);
                    let chan_size = parent_port
                        .and_then(|p| p.chan_size)
                        .or_else(|| port.and_then(|p| p.chan_size))
                        .unwrap_or(0);
                    let conn = connections
                        .entry(parent_arg_name.clone())
                        .or_insert_with(|| MMapConnection {
                            arg_name: parent_arg_name.clone(),
                            id_width: 1,
                            thread_count: 0,
                            args: Vec::new(),
                            chan_count,
                            chan_size,
                            data_width,
                        });
                    conn.thread_count += 1;
                    conn.id_width = conn.id_width.max(child_id_width);
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "instance index fits in u32"
                    )]
                    let idx = inst_idx as u32;
                    conn.args
                        .push((child_task_name.clone(), idx, child_port_name.clone()));
                }
            }
        }

        // Compute parent-facing AXI ID width. Child HLS AXI ports use a
        // 1-bit ID field, and the crossbar appends enough routing bits to
        // identify the originating slave when returning responses.
        for conn in connections.values_mut() {
            conn.id_width = id_width_for_child_threads(conn.id_width, conn.thread_count);
        }

        Ok(connections)
    }

    pub(crate) fn child_mmap_id_width(&self, child_task_name: &str, child_port_name: &str) -> u32 {
        let child_port = sanitize_array_name(child_port_name);
        let prefix = format!("m_axi_{child_port}");
        let module_id_width = self
            .module_map
            .get(child_task_name)
            .and_then(|module| {
                ["_ARID", "_AWID", "_BID", "_RID"]
                    .iter()
                    .filter_map(|suffix| {
                        module
                            .inner
                            .find_port(&format!("{prefix}{suffix}"))
                            .and_then(|port| port_bit_width(port.width.as_ref()))
                    })
                    .max()
            })
            .unwrap_or(1);
        let nested_id_width = self
            .program
            .tasks
            .get(child_task_name)
            .filter(|task| task.level == TaskLevel::Upper)
            .and_then(|_| self.aggregate_mmap_connections(child_task_name).ok())
            .and_then(|conns| conns.get(child_port_name).map(|conn| conn.id_width))
            .unwrap_or(1);
        module_id_width.max(nested_id_width)
    }

    /// Get the top task name.
    pub fn top_task_name(&self) -> &str {
        &self.program.top
    }

    /// Check if a task is upper-level.
    pub fn is_upper_task(&self, task_name: &str) -> bool {
        self.program
            .tasks
            .get(task_name)
            .is_some_and(|t| t.level == TaskLevel::Upper)
    }
}

/// Compute parent-facing AXI ID width: 1 + ceil(log2(n)), minimum 1.
#[cfg(test)]
fn id_width_for_threads(n: u32) -> u32 {
    id_width_for_child_threads(1, n)
}

fn id_width_for_child_threads(child_id_width: u32, n: u32) -> u32 {
    child_id_width.max(1) + routing_id_bits(n)
}

pub fn routing_id_bits(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    u32::BITS - (n - 1).leading_zeros()
}

fn port_bit_width(width: Option<&Width>) -> Option<u32> {
    let Some(width) = width else {
        return Some(1);
    };
    let msb = expression_u32(&width.msb)?;
    let lsb = expression_u32(&width.lsb)?;
    Some(msb.abs_diff(lsb) + 1)
}

fn expression_u32(expr: &Expression) -> Option<u32> {
    expr.iter()
        .map(|token| token.repr.as_str())
        .collect::<String>()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_program() -> Program {
        let json = r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child_a": [{"args": {"data": {"arg": "data", "cat": "istream"}}}]
                    },
                    "fifos": {}
                },
                "child_a": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [{"cat": "istream", "name": "data", "type": "float", "width": 32}],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn attach_module_rejects_unknown_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        let module = VerilogModule::parse("module unknown(); endmodule").unwrap();
        let result = state.attach_module("nonexistent", module);
        assert!(
            matches!(result, Err(CodegenError::TaskNotFound(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn attach_module_rejects_duplicate() {
        let mut state = TopologyWithRtl::new(sample_program());
        let module1 = VerilogModule::parse("module child_a(); endmodule").unwrap();
        let module2 = VerilogModule::parse("module child_a(); endmodule").unwrap();
        state.attach_module("child_a", module1).unwrap();
        let result = state.attach_module("child_a", module2);
        assert!(
            matches!(result, Err(CodegenError::ModuleAlreadyAttached(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn create_fsm_rejects_lower_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        let result = state.create_fsm_module("child_a");
        assert!(
            matches!(result, Err(CodegenError::FsmForLowerTask(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn create_fsm_for_upper_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        state.create_fsm_module("top_task").unwrap();
        assert!(state.fsm_modules.contains_key("top_task"));
    }

    #[test]
    fn id_width_calculation() {
        assert_eq!(id_width_for_threads(0), 1);
        assert_eq!(id_width_for_threads(1), 1);
        assert_eq!(id_width_for_threads(2), 2);
        assert_eq!(id_width_for_threads(3), 3);
        assert_eq!(id_width_for_threads(4), 3);
        assert_eq!(id_width_for_threads(7), 4);
        assert_eq!(id_width_for_threads(8), 4);
    }

    #[test]
    fn aggregate_mmap_connections_preserves_child_axi_id_width() {
        let program = serde_json::from_value(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }))
        .unwrap();
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
            conns["elems"].id_width, 2,
            "parent-facing ID width must be at least as wide as the child AXI port"
        );
    }

    #[test]
    fn aggregate_mmap_connections_preserves_nested_child_crossbar_id_width() {
        let program = serde_json::from_value(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
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
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }))
        .unwrap();
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width, 2,
            "parent-facing ID width should preserve nested child crossbar routing bits"
        );
    }

    #[test]
    fn aggregate_mmap_connections_expands_wide_child_id_for_parent_crossbar() {
        let program = serde_json::from_value(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
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
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
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
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }))
        .unwrap();
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width, 3,
            "parent crossbar should append routing bits to the widest child AXI ID"
        );
    }
}
