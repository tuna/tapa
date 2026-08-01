//! Narrowed per-concern state views (REFACTOR-PLAN §4 Phase 1 item 1b).
//!
//! The pass pipeline never receives the whole [`crate::rtl_state::TopologyWithRtl`];
//! the driver-side context construction splits it once into these disjoint
//! borrow views, one per state concern:
//!
//! - [`DesignView`]: read-only access to the design model and floorplan.
//! - [`ModuleTable`]: mutable access to the attached HLS module table.
//! - [`FsmTable`]: mutable access to the per-task FSM module table.
//! - [`OutputSet`]: mutable access to the emitted-file output maps.
//!
//! Methods rehost here by true concern: single-table mutators (e.g. FSM
//! module creation) move onto their table view; genuine cross-table
//! operations stay on `TopologyWithRtl` for the driver/staging boundary.

use std::borrow::Borrow;
use std::collections::BTreeMap;

use tapa_ir::task::TaskLevel;
use tapa_ir::{Design, FloorplanResult};
use tapa_rtl::mutation::MutableModule;
use tapa_rtl::VerilogModule;

use crate::error::CodegenError;

fn render_fsm_module(fsm_name: &str) -> String {
    format!(
        "\
module {fsm_name} (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle
);
endmodule //{fsm_name}"
    )
}

/// Read-only view of the design model and floorplan result.
#[derive(Clone, Copy)]
pub struct DesignView<'a> {
    design: &'a Design,
    floorplan: Option<&'a FloorplanResult>,
}

impl<'a> DesignView<'a> {
    /// Wrap a design reference and its floorplan result.
    pub fn new(design: &'a Design, floorplan: Option<&'a FloorplanResult>) -> Self {
        Self { design, floorplan }
    }

    /// The design model.
    #[must_use]
    pub fn design(self) -> &'a Design {
        self.design
    }

    /// The floorplan result, when the design has been floorplanned.
    #[must_use]
    pub fn floorplan(self) -> Option<&'a FloorplanResult> {
        self.floorplan
    }
}

/// Mutable view of the attached HLS module table, keyed by task name.
pub struct ModuleTable<'a> {
    map: &'a mut BTreeMap<String, MutableModule>,
}

impl<'a> ModuleTable<'a> {
    /// Wrap the module map.
    pub fn new(map: &'a mut BTreeMap<String, MutableModule>) -> Self {
        Self { map }
    }

    /// Shared lookup by task name.
    #[must_use]
    pub fn get(&self, task_name: &str) -> Option<&MutableModule> {
        self.map.get(task_name)
    }

    /// Mutable lookup by task name.
    pub fn get_mut(&mut self, task_name: &str) -> Option<&mut MutableModule> {
        self.map.get_mut(task_name)
    }

    /// Attach (or replace) the module attached to `task_name`.
    pub fn insert(&mut self, task_name: String, module: MutableModule) -> Option<MutableModule> {
        self.map.insert(task_name, module)
    }

    /// Iterate all attached modules in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &MutableModule)> {
        self.map.iter()
    }
}

impl<Q> std::ops::Index<&'_ Q> for ModuleTable<'_>
where
    String: Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Output = MutableModule;

    fn index(&self, task_name: &Q) -> &Self::Output {
        &self.map[task_name]
    }
}

/// Mutable view of the per-task FSM module table, keyed by task name.
pub struct FsmTable<'a> {
    map: &'a mut BTreeMap<String, MutableModule>,
}

impl<'a> FsmTable<'a> {
    /// Wrap the FSM module map.
    pub fn new(map: &'a mut BTreeMap<String, MutableModule>) -> Self {
        Self { map }
    }

    /// Iterate all FSM modules in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &MutableModule)> {
        self.map.iter()
    }

    /// Create an FSM module for an upper-level task.
    ///
    /// Rejects lower-level tasks. This is the [`FsmTable`]-resident half of
    /// `TopologyWithRtl::create_fsm_module`, which delegates here after the
    /// design-side task lookup.
    pub fn create_fsm_module(
        &mut self,
        task_name: &str,
        task_level: TaskLevel,
    ) -> Result<(), CodegenError> {
        if task_level == TaskLevel::Lower {
            return Err(CodegenError::FsmForLowerTask(task_name.to_owned()));
        }

        // Create an empty FSM module with the standard TAPA handshake ports.
        // The generated top-level RTL wires ap_start / ap_done / ap_ready /
        // ap_idle to this FSM, so they must be present on the FSM module
        // definition.
        let fsm_name = format!("{task_name}_fsm");
        let fsm_source = render_fsm_module(&fsm_name);
        let parsed = VerilogModule::parse(&fsm_source)?;
        self.map
            .insert(task_name.to_owned(), MutableModule::from_parsed(parsed));
        Ok(())
    }
}

/// Mutable view of the emitted-file outputs: generated auxiliary RTL files
/// and the port-only custom RTL templates, both keyed by file path.
pub struct OutputSet<'a> {
    generated_files: &'a mut BTreeMap<String, String>,
    template_files: &'a mut BTreeMap<String, String>,
}

impl<'a> OutputSet<'a> {
    /// Wrap both output maps.
    pub fn new(
        generated_files: &'a mut BTreeMap<String, String>,
        template_files: &'a mut BTreeMap<String, String>,
    ) -> Self {
        Self {
            generated_files,
            template_files,
        }
    }

    /// Insert a generated auxiliary RTL file.
    pub fn insert_generated(&mut self, path: String, rtl: String) -> Option<String> {
        self.generated_files.insert(path, rtl)
    }

    /// Insert a port-only custom RTL template.
    pub fn insert_template(&mut self, path: String, template: String) -> Option<String> {
        self.template_files.insert(path, template)
    }
}
