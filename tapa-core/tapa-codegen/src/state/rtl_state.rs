//! Enriched topology + RTL state for code generation.
//!
//! `TopologyWithRtl` wraps a `Design` with attached Verilog modules
//! parsed from HLS output, plus FSM modules created during codegen.

pub use crate::state::mmap::direct::DirectMmapInterface;
pub use crate::state::mmap::{MMapConnection, MMapSlave};

use std::collections::BTreeMap;

use tapa_ir::task::TaskLevel;
use tapa_ir::{Design, FloorplanResult};
use tapa_rtl::mutation::MutableModule;
use tapa_rtl::VerilogModule;

use crate::error::CodegenError;

/// Enriched state combining topology with RTL modules.
pub struct TopologyWithRtl {
    /// The design model.
    pub design: Design,
    /// The floorplan, when the design has been floorplanned. Its presence
    /// switches codegen onto the pipelined path (Head/Body/Tail cells on
    /// cross-slot streams and matching region constraints).
    pub floorplan: Option<FloorplanResult>,
    /// Parsed HLS Verilog modules, keyed by task name.
    pub module_map: BTreeMap<String, MutableModule>,
    /// FSM modules for upper-level tasks, keyed by task name.
    pub fsm_modules: BTreeMap<String, MutableModule>,
    /// Generated auxiliary RTL files, keyed by file path.
    pub generated_files: BTreeMap<String, String>,
    /// Port-only custom RTL templates, keyed by `<task>.v`.
    pub template_files: BTreeMap<String, String>,
}

impl TopologyWithRtl {
    /// Create a new `TopologyWithRtl` from a `Design`.
    pub fn new(design: Design) -> Self {
        Self {
            design,
            floorplan: None,
            module_map: BTreeMap::new(),
            fsm_modules: BTreeMap::new(),
            generated_files: BTreeMap::new(),
            template_files: BTreeMap::new(),
        }
    }

    /// Whether codegen can emit the distributed controller for the top task.
    ///
    /// This deliberately checks the same upper-task boundary used by
    /// [`crate::generate_rtl`]. Callers preparing a floorplan can use it to
    /// avoid requesting controller hierarchy that codegen would not create.
    #[must_use]
    pub fn supports_distributed_control(&self) -> bool {
        self.design.tasks.get(&self.design.top).is_some_and(|task| {
            task.level == TaskLevel::Upper
                && task.synth != tapa_ir::SynthTarget::Ignore
                && !task.tasks.is_empty()
        }) && self.module_map.contains_key(&self.design.top)
    }

    /// Whether the generated top will instantiate the AXI-Lite control block.
    ///
    /// This is the single read-only predicate shared with the floorplanner.
    /// The pipeline stages it at prepare time for the `s_axi` pass; the
    /// distributed-control plan builder and `tapa-cli` read it directly.
    #[must_use]
    pub fn top_instantiates_control_s_axi(&self) -> bool {
        self.design.tasks.get(&self.design.top).is_some_and(|task| {
            task.level == TaskLevel::Upper && task.synth != tapa_ir::SynthTarget::Ignore
        }) && self.module_map.get(&self.design.top).is_some_and(|module| {
            module
                .inner
                .ports
                .iter()
                .any(|port| port.name == "s_axi_control_AWVALID")
        })
    }

    /// Attach a parsed HLS Verilog module to a task.
    ///
    /// Rejects nonexistent task names and duplicate attachments.
    pub fn attach_module(
        &mut self,
        task_name: &str,
        module: VerilogModule,
    ) -> Result<(), CodegenError> {
        if !self.design.tasks.contains_key(task_name) {
            return Err(CodegenError::TaskNotFound(task_name.to_owned()));
        }
        if self.module_map.contains_key(task_name) {
            return Err(CodegenError::ModuleAlreadyAttached(task_name.to_owned()));
        }
        self.module_map
            .insert(task_name.to_owned(), MutableModule::from_parsed(module));
        Ok(())
    }
}

pub fn routing_id_bits(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    u32::BITS - (n - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_program() -> Design {
        let json = r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "readable_name": "top_task",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "child_a": [{"args": {"data": {"arg": "data", "cat": "istream"}}}]
                    },
                    "fifos": {}
                },
                "child_a": {
                    "readable_name": "child_a",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [{"cat": "istream", "name": "data", "type": "float", "width": 32}],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }"#;
        crate::design_from_fixture_json(serde_json::from_str(json).unwrap())
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
        let result = crate::state::views::FsmTable::new(&mut state.fsm_modules)
            .create_fsm_module("child_a", TaskLevel::Lower);
        assert!(
            matches!(result, Err(CodegenError::FsmForLowerTask(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn create_fsm_for_upper_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        crate::state::views::FsmTable::new(&mut state.fsm_modules)
            .create_fsm_module("top_task", TaskLevel::Upper)
            .unwrap();
        assert!(state.fsm_modules.contains_key("top_task"));
    }
}
