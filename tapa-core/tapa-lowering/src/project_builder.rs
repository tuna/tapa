//! Top-level project assembly: builds a `GraphIR` Project from topology + RTL.

use std::collections::BTreeMap;

use tapa_graphir::{
    AnyModuleDefinition, BaseFields, Expression, GroupedFields, HierarchicalName, ModulePort,
    Modules, Project, interface::AnyInterface,
};
use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_topology::program::Program;

use crate::instantiation_builder::{
    build_arg_table, build_fifo_instance, build_port_connections, build_task_instance,
    ArgTable,
};
use crate::module_defs::{get_fifo_def, get_reset_inverter_def, get_reset_inverter_inst};
use crate::utils::{input_wire, make_wire, range_msb};
use crate::LoweringError;

/// Build a `GraphIR` Project from a `TopologyWithRtl` state.
///
/// This is the lowest-level RTL-bearing entrypoint. It derives leaf modules
/// and FSM modules from the state, and takes the real `{top}_control_s_axi.v`
/// text as input rather than fabricating a placeholder.
///
/// Callers that want the Python-equivalent path boundary should instead use
/// `build_project_from_paths` via `LoweringInputs`.
#[allow(clippy::too_many_lines, reason = "sequential grouped-module post-pass")]
pub fn build_project_from_state(
    state: &TopologyWithRtl,
    ctrl_s_axi_verilog: &str,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
    island_to_pblock_range: Option<BTreeMap<String, Vec<String>>>,
    part_num: Option<String>,
) -> Result<Project, LoweringError> {
    // Derive leaf module definitions from TopologyWithRtl.module_map
    // Lower tasks have their RTL already parsed and attached.
    let mut leaf_modules = BTreeMap::new();
    for (task_name, mm) in &state.module_map {
        if !state.is_upper_task(task_name) {
            leaf_modules.insert(
                task_name.clone(),
                crate::utils::mutable_module_to_verilog_def(mm),
            );
        }
    }

    // Collect parameter lists for upper tasks (top + slots) from their
    // attached RTL. Python's `get_task_graphir_parameters(task_module)` does
    // the same on the upper task's parsed RTL, so the grouped `VecAdd` and
    // each `SLOT_*_SLOT_*` module exposes the parameters the Vitis RTL
    // declares.
    let mut upper_parameters: BTreeMap<String, Vec<tapa_graphir::ModuleParameter>> =
        BTreeMap::new();
    for (task_name, mm) in &state.module_map {
        if state.is_upper_task(task_name) {
            let params: Vec<tapa_graphir::ModuleParameter> = mm
                .inner
                .parameters
                .iter()
                .map(crate::utils::rtl_parameter_to_graphir)
                .collect();
            if !params.is_empty() {
                upper_parameters.insert(task_name.clone(), params);
            }
        }
    }

    // Extract FSM modules from TopologyWithRtl as VerilogModuleDefinitions
    let mut fsm_modules = BTreeMap::new();
    for (name, mm) in &state.fsm_modules {
        let fsm_name = format!("{name}_fsm");
        let ports: Vec<ModulePort> = mm.inner.ports.iter()
            .map(crate::utils::rtl_port_to_graphir)
            .collect();
        fsm_modules.insert(
            fsm_name.clone(),
            AnyModuleDefinition::new_verilog(fsm_name, ports, mm.emit()),
        );
    }

    // Generate ctrl_s_axi module definition with dynamic scalar/MMAP-offset ports
    let ctrl_s_axi_name = format!("{}_control_s_axi", state.program.top);
    let top_task = state.program.tasks.get(&state.program.top);
    let top_ports = top_task.map_or(&[][..], |t| t.ports.as_slice());
    let ctrl_s_axi_def = Some(crate::module_defs::get_ctrl_s_axi_def(
        &ctrl_s_axi_name,
        ctrl_s_axi_verilog,
        top_ports,
    ));

    let mut project = build_project(
        &state.program,
        &leaf_modules,
        &fsm_modules,
        ctrl_s_axi_def,
        slot_to_instances,
        island_to_pblock_range,
        part_num,
        Some(state),
    )?;

    // Inject upper-task parameter lists onto the corresponding grouped
    // module definitions. Grouped modules constructed by build_project
    // default to empty parameters; Python's `VecAdd` / `SLOT_*_SLOT_*`
    // grouped modules carry the parameter declarations from the Vitis RTL.
    for module in &mut project.modules.module_definitions {
        if let AnyModuleDefinition::Grouped { base, .. } = module {
            if let Some(params) = upper_parameters.get(&base.name) {
                if base.parameters.is_empty() {
                    base.parameters.clone_from(params);
                }
            }
        }
    }

    // Replace the synthesized top-module port list with the parsed top
    // RTL's own ports, matching Python's `get_task_graphir_ports(top.rtl_module)`
    // in `gen_rs_graphir.get_top_module_definition`. This removes synthetic
    // ports the topology-based expansion adds but the Vitis top RTL does
    // not (e.g. `a`, `b`, `c` scalar offsets, `*_offset`, `*_ARREGION`).
    //
    // Applied unconditionally when the top RTL is attached — callers must
    // supply a Vitis-complete top RTL (declaring `ap_clk`, `ap_rst_n`, and
    // the `s_axi_control_*` AXI-Lite ports that the `ctrl_s_axi`
    // instantiation binds to) so DRC stays clean.
    // Initial top-port replacement with parsed top RTL. The top-wire
    // rewrite is deferred until after slot grouped modules have been
    // rewritten, so Python's `get_upper_task_ir_wires(top, slot_defs, ...)`
    // can use the finalized slot defs for FIFO data-range inference.
    if let Some(top_mm) = state.module_map.get(&state.program.top) {
        let top_rtl_ports: Vec<ModulePort> = top_mm
            .inner
            .ports
            .iter()
            .map(crate::utils::rtl_port_to_graphir)
            .collect();
        let top_port_names: std::collections::BTreeSet<String> =
            top_rtl_ports.iter().map(|p| p.name.clone()).collect();
        if let Some((base, grouped)) =
            find_grouped_mut(&mut project.modules.module_definitions, &state.program.top)
        {
            base.ports.clone_from(&top_rtl_ports);
            // Drop any wire now declared as a top port to avoid duplicate
            // identifiers; the full Python-equivalent wire list is
            // installed after the slot rewrite below.
            grouped.wires.retain(|w| !top_port_names.contains(&w.name));
        }
    }

    // Replace slot grouped-module port lists with the Python-equivalent
    // output. Mirrors `tapa/graphir_conversion/pipeline/ports_builder.py::
    // get_slot_module_definition_ports`:
    //   * For each slot port, find a child instance whose arg.arg equals
    //     the slot port name (Python's `_find_port_child`).
    //   * Derive slot-visible ports from the child port category via the
    //     Rust equivalent of `get_child_port_connection_mapping`:
    //     - scalar → `{child_port: arg}`
    //     - i/ostream → for each suffix in ISTREAM/OSTREAM_SUFFIXES,
    //       look up the child RTL port via `VerilogModule::get_port_of`
    //       (handles `_V`/`_r`/`_s`/bare infix + singleton array) and
    //       emit `{arg}{suffix}` at the slot boundary.
    //     - mmap → always emit `{arg}_offset`; for every M-AXI suffix
    //       the child RTL declares, emit `m_axi_{arg}{suffix}`.
    //   * Port direction/range come from the child leaf IR's matching
    //     port entry.
    //   * Append handshake ports (ap_clk, ap_rst_n, ap_start, ap_done,
    //     ap_ready, ap_idle).
    // Slot ports whose names don't match any child arg are skipped,
    // mirroring Python's skip behavior.
    let slot_names: Vec<String> = project
        .modules
        .module_definitions
        .iter()
        .filter_map(|m| {
            if let AnyModuleDefinition::Grouped { base, .. } = m {
                if base.name != state.program.top
                    && state.program.tasks.contains_key(&base.name)
                {
                    return Some(base.name.clone());
                }
            }
            None
        })
        .collect();
    for slot_name in slot_names {
        let Some(new_ports) = crate::slot_ports::build_slot_ports_python_equivalent(
            &slot_name,
            state,
            &leaf_modules,
        ) else {
            continue;
        };
        let Some(slot_task) = state.program.tasks.get(&slot_name) else {
            continue;
        };
        // Python-equivalent slot wires from `get_upper_task_ir_wires`.
        // We do NOT auto-declare wires for Vitis FSM RTL ports the Python
        // wire builder never emits — parity with Python's strict
        // `get_upper_task_ir_wires` output is the contract.
        let new_wires = crate::upper_wires::build_upper_task_ir_wires(
            slot_task,
            &new_ports,
            &[],
            &leaf_modules,
        );
        if let Some((base, grouped)) =
            find_grouped_mut(&mut project.modules.module_definitions, &slot_name)
        {
            base.ports.clone_from(&new_ports);
            grouped.wires.clone_from(&new_wires);
        }
    }

    // Top-wire rewrite: must run AFTER slots are finalized so
    // `build_upper_task_ir_wires` sees the slot grouped defs as the
    // top task's submodule IR defs (Python passes `slot_defs` here,
    // not leaf defs — top-level FIFOs are produced/consumed by slots).
    let top_name = &state.program.top;
    if state.module_map.contains_key(top_name) {
        if let Some(top_task) = state.program.tasks.get(top_name) {
            // Build a merged `ir_defs` map: slot grouped defs plus leaf
            // defs. The wire builder's FIFO data-range inference walks
            // the producer's IR-def ports; top FIFOs' producers are
            // slots, so including slot defs is what makes the range
            // match Python.
            let mut ir_defs: BTreeMap<String, AnyModuleDefinition> = BTreeMap::new();
            for module in &project.modules.module_definitions {
                if let AnyModuleDefinition::Grouped { base, .. } = module {
                    if base.name != *top_name {
                        ir_defs.insert(base.name.clone(), module.clone());
                    }
                }
            }
            for (name, def) in &leaf_modules {
                ir_defs.entry(name.clone()).or_insert_with(|| def.clone());
            }

            let top_rtl_ports: Vec<ModulePort> = project
                .modules
                .module_definitions
                .iter()
                .find_map(|m| {
                    if let AnyModuleDefinition::Grouped { base, .. } = m {
                        if base.name == *top_name {
                            return Some(base.ports.clone());
                        }
                    }
                    None
                })
                .unwrap_or_default();
            let ctrl_s_axi_name = format!("{top_name}_control_s_axi");
            let ctrl_s_axi_ports: Vec<ModulePort> = project
                .modules
                .module_definitions
                .iter()
                .find(|m| m.name() == ctrl_s_axi_name)
                .map(|m| m.ports().to_vec())
                .unwrap_or_default();
            let mut new_wires = crate::upper_wires::build_upper_task_ir_wires(
                top_task,
                &top_rtl_ports,
                &ctrl_s_axi_ports,
                &ir_defs,
            );
            new_wires.extend(crate::upper_wires::build_top_extra_wires(
                &ctrl_s_axi_ports,
            ));
            new_wires.push(crate::utils::make_wire("rst", None));
            if let Some((_, grouped)) =
                find_grouped_mut(&mut project.modules.module_definitions, top_name)
            {
                grouped.wires.clone_from(&new_wires);
            }
        }
    }

    // Aggregate slot module parameters from each slot's child leaf RTL —
    // matches Python's `get_slot_module_definition_parameters`. For every
    // slot task, walk the child leaf tasks (via the slot's `tasks`
    // dictionary), collect their RTL parameter lists from
    // `state.module_map`, and dedupe by name.
    aggregate_slot_leaf_parameters(
        &mut project,
        state,
        slot_to_instances,
    );

    // Rebuild slot-module interfaces on the finalized slot-port lists.
    // `build_project` runs before `build_slot_ports_python_equivalent`,
    // so its slot ifaces reflect the topology-synthesized port list —
    // which includes internal FIFO signals (e.g. `b_q_VecAdd_din` on
    // SLOT_X3Y3) that Python never exposes on the slot boundary and
    // therefore never emits a handshake iface for. Rebuilding only the
    // slot entries drops the stale handshakes without touching the top
    // or infrastructure module ifaces (whose port basis did not
    // change).
    let fresh_slot_ifaces = build_interfaces(
        &project.modules.module_definitions,
        &state.program,
        slot_to_instances,
    );
    if let Some(existing) = project.ifaces.as_mut() {
        let slot_defs_only: Vec<AnyModuleDefinition> = project
            .modules
            .module_definitions
            .iter()
            .filter(|m| slot_to_instances.contains_key(m.name()))
            .cloned()
            .collect();
        for slot_name in slot_to_instances.keys() {
            if let Some(ifs) = fresh_slot_ifaces.get(slot_name) {
                let mut ifaces_only = BTreeMap::new();
                ifaces_only.insert(slot_name.clone(), ifs.clone());
                crate::iface_roles::apply_iface_roles(&slot_defs_only, &mut ifaces_only)?;
                if let Some(updated) = ifaces_only.remove(slot_name) {
                    existing.insert(slot_name.clone(), updated);
                }
            }
        }
    }

    Ok(project)
}

/// Aggregate leaf RTL parameters onto slot grouped modules.
///
/// Mirrors Python's `get_slot_module_definition_parameters(leaf_ir_defs)`:
/// iterate every leaf module's parameters in the SAME order Python does
/// and keep the first-seen `ModuleParameter` for each name verbatim.
/// Python's `Task.__init__` in `tapa/task.py` calls
/// `dict(sorted(tasks.items()))` on each upper task's children, so
/// `task.instances` (built from this sorted dict in
/// `tapa/program/rtl_codegen.py`) iterates child task names
/// alphabetically. `leaf_ir_defs` is built by walking
/// `top_task.instances` → each slot's `slot_task.instances`, both
/// alphabetical-by-task-name. Our `BTreeMap<String, Vec<InstanceDesign>>`
/// iteration matches this.
///
/// For the `VecAdd` shared fixture this produces `Mmap2Stream` (in
/// `SLOT_X0Y2`, whose alphabetical name starts with `SLOT_X0`) as the
/// first-seen leaf — winning `ap_ST_fsm_state*` = `10'd*` over `Add`'s
/// `3'd*`. For a slot with `zleaf` listed before `aleaf` in the raw
/// JSON, Python sorts them so `aleaf` wins (alphabetical).
///
/// Since `leaf_ir_defs` is the full project leaf set, every slot ends
/// up with the same parameter list; we compute it once and apply it to
/// each slot module.
fn aggregate_slot_leaf_parameters(
    project: &mut Project,
    state: &TopologyWithRtl,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
) {
    let top_name = &state.program.top;
    let Some(top_task) = state.program.tasks.get(top_name) else {
        return;
    };

    // Python-equivalent iteration: for each slot (in top.tasks order),
    // for each leaf (in slot.tasks order), collect RTL parameters,
    // preserving the first `ModuleParameter` seen for each name.
    let mut aggregated: Vec<tapa_graphir::ModuleParameter> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let push_params_of = |task_name: &str,
                              aggregated: &mut Vec<tapa_graphir::ModuleParameter>,
                              seen: &mut std::collections::BTreeSet<String>| {
        let Some(mm) = state.module_map.get(task_name) else {
            return;
        };
        for rtl_param in &mm.inner.parameters {
            if seen.insert(rtl_param.name.clone()) {
                aggregated.push(crate::utils::rtl_parameter_to_graphir(rtl_param));
            }
        }
    };
    for slot_name in top_task.tasks.keys() {
        let Some(slot_task) = state.program.tasks.get(slot_name) else {
            continue;
        };
        for leaf_name in slot_task.tasks.keys() {
            if state.is_upper_task(leaf_name) {
                continue;
            }
            push_params_of(leaf_name, &mut aggregated, &mut seen);
        }
    }

    if aggregated.is_empty() {
        return;
    }

    for module in &mut project.modules.module_definitions {
        let AnyModuleDefinition::Grouped { base, .. } = module else {
            continue;
        };
        if !slot_to_instances.contains_key(&base.name) {
            continue;
        }
        let existing: std::collections::BTreeSet<String> =
            base.parameters.iter().map(|p| p.name.clone()).collect();
        for param in &aggregated {
            if !existing.contains(&param.name) {
                base.parameters.push(param.clone());
            }
        }
    }
}

/// Build a `GraphIR` Project with the `{top}_control_s_axi.v` RTL
/// source supplied explicitly.
///
/// Callers must supply the real `ctrl_s_axi` Verilog text. Emitting a
/// placeholder body leaked through the exporter as a `.v` file with
/// no `module ... endmodule`, which downstream tools rejected as
/// invalid Verilog — so this entrypoint requires the real source up
/// front. Use [`build_project_from_paths`] when the source lives on
/// disk.
///
/// # Errors
///
/// Returns [`LoweringError::MissingCtrlSAxi`] when `ctrl_s_axi_verilog`
/// is empty or lacks a `module` declaration.
pub fn build_project_from_inputs(
    state: &TopologyWithRtl,
    ctrl_s_axi_verilog: &str,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
    island_to_pblock_range: Option<BTreeMap<String, Vec<String>>>,
    part_num: Option<String>,
) -> Result<Project, LoweringError> {
    if ctrl_s_axi_verilog.trim().is_empty()
        || !ctrl_s_axi_verilog.contains("module")
    {
        return Err(LoweringError::MissingCtrlSAxi(format!(
            "no `{}_control_s_axi` RTL source provided; pass the real \
             Verilog via ctrl_s_axi_verilog or use build_project_from_paths",
            state.program.top
        )));
    }
    build_project_from_state(
        state,
        ctrl_s_axi_verilog,
        slot_to_instances,
        island_to_pblock_range,
        part_num,
    )
}

/// Build a `GraphIR` Project from `LoweringInputs`.
///
/// Reads `floorplan.json`, `device_config.json`, and `{top}_control_s_axi.v`
/// from disk. Matches the Python `get_project_from_floorplanned_program`
/// boundary.
///
/// # Errors
///
/// - `LoweringError::MissingCtrlSAxi` if `{top}_control_s_axi.v` is absent.
/// - `LoweringError::MissingLeafRtl` if any leaf task's `.v` file is absent
///   (only enforced for leaf tasks that are not already attached in the
///   `TopologyWithRtl`).
/// - `LoweringError::Json` / `LoweringError::Io` for malformed config files.
pub fn build_project_from_paths(
    inputs: crate::LoweringInputs<'_>,
) -> Result<Project, LoweringError> {
    let crate::LoweringInputs {
        state,
        device_config,
        floorplan,
        rtl_dir,
    } = inputs;

    // Identify leaf tasks that the program references but that are not yet
    // attached to the TopologyWithRtl. Parse each one from `rtl_dir/{name}.v`
    // and attach to the state so downstream `build_project_from_state` can
    // derive a real `leaf_modules` map.
    let leaf_task_names: Vec<String> = state
        .program
        .tasks
        .keys()
        .filter(|name| !state.is_upper_task(name) && !state.module_map.contains_key(*name))
        .cloned()
        .collect();
    for name in &leaf_task_names {
        let path = rtl_dir.join(format!("{name}.v"));
        let body = std::fs::read_to_string(&path)
            .map_err(|_| LoweringError::MissingLeafRtl(path.display().to_string()))?;
        let module = tapa_rtl::VerilogModule::parse(&body).map_err(|e| {
            LoweringError::MissingLeafRtl(format!("{}: {e}", path.display()))
        })?;
        state
            .attach_module(name, module)
            .map_err(|e| LoweringError::MissingLeafRtl(format!("{name}: {e}")))?;
    }

    // Attach real FSM RTL for every upper task whose `{task}_fsm.v`
    // exists on disk. This lets lowering produce a GraphIR project
    // whose FSM module definitions carry the full Vitis port list
    // (ap_start, ap_done, slot-prefixed handshake ports, …) instead
    // of the 6-port stub the fallback `create_fsm_module`
    // synthesizes. Matches Python's
    // `get_fsm_def(program.get_rtl_path(task.rtl_fsm_module.name))`.
    //
    // Missing or malformed FSM RTL is surfaced as
    // `LoweringError::MissingFsmRtl` rather than silently falling
    // back to the 6-port stub — otherwise downstream wiring / iface
    // parity would silently diverge from Python with the root cause
    // hidden.
    let upper_task_names: Vec<String> = state
        .program
        .tasks
        .keys()
        .filter(|n| state.is_upper_task(n))
        .cloned()
        .collect();
    for task_name in &upper_task_names {
        let fsm_path = rtl_dir.join(format!("{task_name}_fsm.v"));
        let body = std::fs::read_to_string(&fsm_path)
            .map_err(|_| LoweringError::MissingFsmRtl(fsm_path.display().to_string()))?;
        let module = tapa_rtl::VerilogModule::parse(&body).map_err(|e| {
            LoweringError::MissingFsmRtl(format!("{}: {e}", fsm_path.display()))
        })?;
        // Drop any stub that was already attached so the real RTL wins.
        state.fsm_modules.remove(task_name);
        state
            .fsm_modules
            .insert(task_name.clone(), tapa_rtl::mutation::MutableModule::from_parsed(module));
    }

    // Read the config inputs using the bundled paths. `?` propagates both
    // missing-file and malformed-JSON errors instead of silently producing
    // `None` values.
    let ctrl_s_axi_path = rtl_dir.join(format!("{}_control_s_axi.v", state.program.top));
    let ctrl_s_axi_body = std::fs::read_to_string(&ctrl_s_axi_path)
        .map_err(|_| LoweringError::MissingCtrlSAxi(ctrl_s_axi_path.display().to_string()))?;

    // Derive slot-to-instances. When the program's topology carries
    // `slot_task_name_to_fp_region`, use the slot-task hierarchy as
    // authoritative (matching Python's pre-baked slot task names like
    // `SLOT_X0Y2_SLOT_X0Y2`). Otherwise fall back to floorplan-region
    // derivation.
    let slot_to_instances = if state.program.slot_task_name_to_fp_region.is_some() {
        slot_to_instances_from_topology(&state.program)
    } else {
        read_slot_to_instances(&floorplan)?
    };
    let (pblock_ranges, part_num) = read_device_config(&device_config, &floorplan)?;

    build_project_from_state(
        state,
        &ctrl_s_axi_body,
        &slot_to_instances,
        Some(pblock_ranges),
        part_num,
    )
}

/// Derive slot → instance mapping from the pre-baked slot-task hierarchy in
/// the program. Slot module names are the slot task names themselves (e.g.,
/// `SLOT_X0Y2_SLOT_X0Y2`), and each slot's instances come from its child
/// task definitions. Matches Python's `_build_program` convention.
fn slot_to_instances_from_topology(
    program: &Program,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(region_map) = program.slot_task_name_to_fp_region.as_ref() else {
        return out;
    };
    for slot_task_name in region_map.keys() {
        let Some(slot_task) = program.tasks.get(slot_task_name) else {
            continue;
        };
        let mut instances: Vec<String> = slot_task
            .tasks
            .iter()
            .flat_map(|(child_task_name, insts)| {
                (0..insts.len()).map(move |idx| format!("{child_task_name}_{idx}"))
            })
            .collect();
        instances.sort();
        out.insert(slot_task_name.clone(), instances);
    }
    out
}

/// Parse `floorplan.json` into a slot → instance mapping (colons collapsed to underscores).
fn read_slot_to_instances(
    floorplan: &std::path::Path,
) -> Result<BTreeMap<String, Vec<String>>, LoweringError> {
    let text = std::fs::read_to_string(floorplan)
        .map_err(|_| LoweringError::PathNotFound(floorplan.display().to_string()))?;
    let vertex_to_region: BTreeMap<String, String> = serde_json::from_str(&text)?;
    let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (vertex, region) in vertex_to_region {
        let slot_name = region.replace(':', "_");
        slot_to_insts.entry(slot_name).or_default().push(vertex);
    }
    for insts in slot_to_insts.values_mut() {
        insts.sort();
    }
    Ok(slot_to_insts)
}

/// `device_config` + `floorplan` join result: pblock map plus part number.
pub type IslandPblockMap = BTreeMap<String, Vec<String>>;

#[derive(serde::Deserialize)]
struct DeviceConfigFile {
    #[serde(default)]
    slots: Vec<DeviceSlotEntry>,
    #[serde(default)]
    part_num: Option<String>,
}

#[derive(serde::Deserialize)]
struct DeviceSlotEntry {
    x: u32,
    y: u32,
    #[serde(default)]
    pblock_ranges: Vec<String>,
}

/// Parse `device_config.json` + `floorplan.json` into the pblock range map
/// plus the FPGA part number. Failures (missing / malformed files) surface
/// as `LoweringError` instead of being silently swallowed.
fn read_device_config(
    device_config: &std::path::Path,
    floorplan: &std::path::Path,
) -> Result<(IslandPblockMap, Option<String>), LoweringError> {
    let device_text = std::fs::read_to_string(device_config)
        .map_err(|_| LoweringError::PathNotFound(device_config.display().to_string()))?;
    let floorplan_text = std::fs::read_to_string(floorplan)
        .map_err(|_| LoweringError::PathNotFound(floorplan.display().to_string()))?;
    let device: DeviceConfigFile = serde_json::from_str(&device_text)?;
    let floorplan_map: BTreeMap<String, String> = serde_json::from_str(&floorplan_text)?;
    let used_slots: std::collections::HashSet<String> = floorplan_map.into_values().collect();

    let mut out = BTreeMap::new();
    for slot in device.slots {
        let canonical = format!("SLOT_X{x}Y{y}:SLOT_X{x}Y{y}", x = slot.x, y = slot.y);
        if !used_slots.contains(&canonical) {
            continue;
        }
        let key = canonical.replace(':', "_TO_");
        out.insert(key, slot.pblock_ranges);
    }
    Ok((out, device.part_num))
}

/// Build a `GraphIR` Project from a floorplanned program.
///
/// Lower-level entrypoint accepting pre-extracted components. Prefer
/// `build_project_from_inputs` when working with `LoweringInputs`.
/// It assembles:
/// - Leaf module definitions (from RTL files)
/// - Slot grouped module definitions
/// - Top grouped module definition
/// - FIFO template definition
/// - FSM module definitions
/// - Reset inverter definition
/// - `ctrl_s_axi` definition
#[allow(
    clippy::too_many_arguments,
    reason = "lower-level entrypoint; prefer build_project_from_state"
)]
pub fn build_project(
    program: &Program,
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
    fsm_modules: &BTreeMap<String, AnyModuleDefinition>,
    ctrl_s_axi_def: Option<AnyModuleDefinition>,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
    island_to_pblock_range: Option<BTreeMap<String, Vec<String>>>,
    part_num: Option<String>,
    state: Option<&TopologyWithRtl>,
) -> Result<Project, LoweringError> {
    let top = program
        .tasks
        .get(&program.top)
        .ok_or_else(|| LoweringError::MissingModule(program.top.clone()))?;

    let mut module_defs: Vec<AnyModuleDefinition> = Vec::new();

    // Add leaf module definitions
    for def in leaf_modules.values() {
        module_defs.push(def.clone());
    }

    // Add FIFO template definition
    module_defs.push(get_fifo_def());

    // Add reset inverter definition
    module_defs.push(get_reset_inverter_def());

    // Add FSM module definitions
    for def in fsm_modules.values() {
        module_defs.push(def.clone());
    }

    // Add ctrl_s_axi if present (check before move)
    let has_ctrl_s_axi = ctrl_s_axi_def.is_some();
    if let Some(ctrl) = ctrl_s_axi_def {
        module_defs.push(ctrl);
    }

    // Build arg table for pipeline signal routing
    let arg_table = build_arg_table(top);

    // Build slot module definitions (collect for top module connection)
    let mut slot_defs = Vec::new();
    for (slot_name, inst_names) in slot_to_instances {
        let slot_def = build_slot_module(
            program,
            slot_name,
            inst_names,
            leaf_modules,
            fsm_modules,
            &arg_table,
            state,
        );
        slot_defs.push(slot_def);
    }
    module_defs.extend(slot_defs.iter().cloned());

    // Build top module definition with FSM and ctrl_s_axi instances.
    // Pre-compute top RTL parameters (same ones the post-pass injects into
    // the grouped top base.parameters) so `control_s_axi_U` can copy the
    // actual parameter expressions, matching Python's
    // `Expression(top_param_by_name[value].expr.root)`.
    let fsm_name = format!("{}_fsm", program.top);
    let top_rtl_params: Vec<tapa_graphir::ModuleParameter> = state
        .and_then(|s| s.module_map.get(&program.top))
        .map(|mm| {
            mm.inner
                .parameters
                .iter()
                .map(crate::utils::rtl_parameter_to_graphir)
                .collect()
        })
        .unwrap_or_default();
    let top_def = build_top_module(
        program,
        top,
        slot_to_instances,
        &slot_defs,
        fsm_modules,
        &fsm_name,
        has_ctrl_s_axi,
        &top_rtl_params,
        leaf_modules,
    );
    module_defs.push(top_def);

    // Sort module definitions by name for deterministic output
    module_defs.sort_by(|a, b| a.name().cmp(b.name()));

    // Build interfaces and apply SOURCE/SINK roles before moving module_defs.
    let mut ifaces = build_interfaces(&module_defs, program, slot_to_instances);
    crate::iface_roles::apply_iface_roles(&module_defs, &mut ifaces)?;

    Ok(Project {
        part_num,
        modules: Modules {
            name: "$root".to_owned(),
            module_definitions: module_defs,
            top_name: Some(program.top.clone()),
        },
        blackboxes: Vec::new(),
        ifaces: Some(ifaces),
        module_to_rtl_pragmas: None,
        module_to_old_rtl_pragmas: None,
        island_to_pblock_range,
        routes: None,
        resource_to_max_local_usage: None,
        cut_to_crossing_count: None,
        extra: BTreeMap::new(),
    })
}

/// Build a slot module definition containing task instances and FIFOs.
#[allow(clippy::too_many_lines, reason = "sequential slot assembly logic")]
fn build_slot_module(
    program: &Program,
    slot_name: &str,
    inst_names: &[String],
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
    fsm_modules: &BTreeMap<String, AnyModuleDefinition>,
    arg_table: &ArgTable,
    state: Option<&TopologyWithRtl>,
) -> AnyModuleDefinition {
    // Physical floorplan region for child instances: when the slot task
    // is pre-baked (slot_task_name_to_fp_region maps slot_name → region),
    // use the region string verbatim ("SLOT_X0Y0:SLOT_X0Y0"). Otherwise
    // fall back to the slot name. This matches Python's
    // program.slot_task_name_to_fp_region lookup.
    let fp_region = program
        .slot_task_name_to_fp_region
        .as_ref()
        .and_then(|m| m.get(slot_name).cloned())
        .unwrap_or_else(|| slot_name.to_owned());
    // Slot-local arg table for this slot's children. Mirrors Python's
    // `get_task_arg_table(slot)` used in instantiation_builder.py
    // for slot grouped modules — Rust previously built arg tables from
    // the top task, which means child leaf instances inside a slot had
    // no arg entries and `build_port_connections` fell back to raw arg
    // names instead of Python's `{inst}___{arg}[_offset]__q0` queue-tail
    // signals. When a slot_name does not correspond to a registered
    // program task (small test fixtures), fall back to the top task for
    // the arg-table context so the builder still produces a compatible
    // shape.
    let top = &program.tasks[&program.top];
    let slot_task_ref = program.tasks.get(slot_name).unwrap_or(top);
    let slot_arg_table = crate::instantiation_builder::build_arg_table(slot_task_ref);
    let mut ports = vec![
        input_wire("ap_clk", None),
        input_wire("ap_rst_n", None),
        input_wire("ap_start", None),
        crate::utils::output_wire("ap_done", None),
        crate::utils::output_wire("ap_idle", None),
        crate::utils::output_wire("ap_ready", None),
    ];
    let mut submodules = Vec::new();
    let mut wires = Vec::new();

    // Python's slot grouped modules do not contain a reset_inverter
    // instance; the reset_inverter is a top-level instance only. Keep the
    // `ap_rst` wire declaration for local signals that reference it, but
    // skip the per-slot instantiation.
    wires.push(make_wire("ap_rst", None));

    // Add pipeline wires from arg table for instances in this slot
    for inst_name in inst_names {
        if let Some(inst_signals) = arg_table.get(inst_name) {
            for signal in inst_signals.values() {
                if !wires.iter().any(|w| w.name == *signal) {
                    wires.push(make_wire(signal, None));
                }
            }
        }
    }

    // Add task instances belonging to this slot
    for inst_name in inst_names {
        // Parse "taskname_idx" format
        let (task_name, _idx) = parse_instance_name(inst_name);
        if let Some(task) = program.tasks.get(&task_name) {
            // Add control wires for this instance
            for suffix in &["__ap_start", "__ap_done", "__ap_idle", "__ap_ready"] {
                wires.push(make_wire(&format!("{inst_name}{suffix}"), None));
            }

            // Collect child RTL port names for MMAP filtering
            let child_rtl_ports: Option<std::collections::HashSet<String>> =
                leaf_modules.get(&task_name).map(|def| {
                    def.ports().iter().map(|p| p.name.clone()).collect()
                });

            // Find the instance's args in the SLOT task, using the
            // slot-local arg table for pipeline routing. Mirrors Python's
            // `get_upper_module_ir_subinsts(slot, ...)` which walks
            // `slot.instances`, not the top's.
            let inst_arg_table = slot_arg_table.get(inst_name);
            let mut arg_connections = Vec::new();
            if let Some(instances) = slot_task_ref.tasks.get(&task_name) {
                for (idx, inst) in instances.iter().enumerate() {
                    let expected_name = format!("{task_name}_{idx}");
                    if expected_name == *inst_name {
                        for (port_name, arg) in &inst.args {
                            // Connect child port through the slot-local arg
                            // table for all categories. Scalars route through
                            // queue-tail wires ({inst}___{arg}__q0); mmap
                            // offsets through ({inst}___{arg}_offset__q0),
                            // matching Python's `_connect_scalar` +
                            // `_connect_mmap_offset` + FIFO-handshake flows.
                            let child_rtl_ref = state
                                .and_then(|s| s.module_map.get(&task_name))
                                .map(|mm| &mm.inner);
                            let slot_conns = build_port_connections(
                                port_name,
                                arg,
                                inst_arg_table,
                                child_rtl_ports.as_ref(),
                                child_rtl_ref,
                            );
                            arg_connections.extend(slot_conns);

                            // Expose slot ports using the PARENT-VISIBLE arg name
                            if matches!(
                                arg.cat,
                                tapa_task_graph::port::ArgCategory::Scalar
                            ) {
                                let width = task.ports.iter()
                                    .find(|p| p.name == *port_name)
                                    .map_or(32, |p| p.width);
                                let port_def = input_wire(&arg.arg, port_range(width));
                                if !ports.iter().any(|p: &ModulePort| p.name == port_def.name) {
                                    ports.push(port_def);
                                }
                            }
                        }
                    }
                }
            }

            // Expand stream/mmap ports to RTL-level signals
            for port in &task.ports {
                if matches!(port.cat, tapa_task_graph::port::ArgCategory::Scalar) {
                    continue;
                }
                // Use the arg name (parent-visible) for port expansion.
                // Look in the slot task's own task map first (pre-baked slot
                // hierarchy), then fall back to the top task's map. Python's
                // `_find_port_child` walks `slot.instances`; we mirror that
                // by preferring the slot task's own `tasks` dict.
                let arg_name =
                    find_arg_name_in_task(slot_task_ref, &task_name, inst_name, &port.name)
                        .unwrap_or_else(|| port.name.clone());
                if matches!(
                    port.cat,
                    tapa_task_graph::port::ArgCategory::Mmap
                    | tapa_task_graph::port::ArgCategory::AsyncMmap
                    | tapa_task_graph::port::ArgCategory::Immap
                    | tapa_task_graph::port::ArgCategory::Ommap
                ) {
                    // For MMAP: filter AXI channels against child RTL ports
                    if let Some(ref known) = child_rtl_ports {
                        // Offset port always present
                        let offset_name = format!("{arg_name}_offset");
                        if !ports.iter().any(|p: &ModulePort| p.name == offset_name) {
                            ports.push(input_wire(&offset_name, Some(range_msb(63))));
                        }
                        // Only emit AXI channels that exist in the child RTL
                        for &suffix in crate::utils::M_AXI_READ_SUFFIXES
                            .iter()
                            .chain(crate::utils::M_AXI_WRITE_SUFFIXES.iter())
                        {
                            let child_port = crate::utils::m_axi_port_name(&port.name, suffix);
                            if !known.contains(&child_port) {
                                continue;
                            }
                            let slot_port = crate::utils::m_axi_port_name(&arg_name, suffix);
                            if !ports.iter().any(|p: &ModulePort| p.name == slot_port) {
                                // Slot port direction matches child RTL port direction.
                                let is_child_output = leaf_modules.get(&task_name)
                                    .and_then(|def| def.ports().iter().find(|p| p.name == child_port))
                                    .is_some_and(ModulePort::is_output);
                                let port = if is_child_output {
                                    crate::utils::output_wire(&slot_port, None)
                                } else {
                                    input_wire(&slot_port, None)
                                };
                                ports.push(port);
                            }
                        }
                        continue;
                    }
                }
                // For streams (and MMAP without RTL info): use static expansion
                for expanded in crate::utils::expand_port_to_signals(&arg_name, port.cat, port.width) {
                    if !ports.iter().any(|p: &ModulePort| p.name == expanded.name) {
                        ports.push(expanded);
                    }
                }
            }

            submodules.push(build_task_instance(
                inst_name,
                &task_name,
                arg_connections,
                Some(&fp_region),
            ));
        }
    }

    // Add the slot FSM instance unconditionally. Python's
    // `get_upper_module_ir_subinsts` appends
    // `_make_fsm_inst(upper_task.rtl_fsm_module, floorplan_region)` at
    // this point — a self-connected instance named `{slot}_fsm_0` that
    // references every FSM port. Any FSM port that isn't already a slot
    // port or wire gets added as a wire so the exporter's DRC can find
    // every identifier.
    let slot_fsm_name = format!("{slot_name}_fsm");
    if let Some(fsm_def) = fsm_modules.get(&slot_fsm_name) {
        for p in fsm_def.ports() {
            let already_declared = ports.iter().any(|port| port.name == p.name)
                || wires.iter().any(|w| w.name == p.name);
            if !already_declared {
                wires.push(make_wire(&p.name, p.range.clone()));
            }
        }
        let fsm_connections: Vec<tapa_graphir::ModuleConnection> = fsm_def
            .ports()
            .iter()
            .map(|p| crate::utils::make_connection(&p.name, Expression::new_id(&p.name)))
            .collect();
        submodules.push(tapa_graphir::ModuleInstantiation {
            name: format!("{slot_fsm_name}_0"),
            hierarchical_name: HierarchicalName::get_name(&format!("{slot_fsm_name}_0")),
            module: slot_fsm_name.clone(),
            connections: fsm_connections,
            parameters: Vec::new(),
            floorplan_region: Some(fp_region.clone()),
            area: None,
            pragmas: Vec::new(),
            extra: BTreeMap::default(),
        });
    }

    // Add FIFO instances for FIFOs whose producer and consumer both live
    // inside this slot. Python's `get_upper_module_ir_subinsts` iterates
    // `upper_task.fifos` (the slot's own FIFO map, not the top task's)
    // and keeps only the internal ones via `is_fifo_external_codegen`.
    // Intra-slot FIFOs are those with both `produced_by` and `consumed_by`
    // set.
    if let Some(slot_task) = program.tasks.get(slot_name) {
        for (fifo_name, fifo) in &slot_task.fifos {
            if fifo.produced_by.is_none() || fifo.consumed_by.is_none() {
                continue;
            }
            // Ensure wire declarations for the FIFO data/handshake signals
            // so the exporter's DRC finds every identifier referenced by
            // the FIFO instance's connection list.
            for suffix in ["_din", "_dout", "_empty_n", "_full_n", "_read", "_write"] {
                let wire_name = format!("{fifo_name}{suffix}");
                if !ports.iter().any(|p| p.name == wire_name)
                    && !wires.iter().any(|w| w.name == wire_name)
                {
                    wires.push(make_wire(&wire_name, None));
                }
            }
            let depth = fifo.depth.unwrap_or(32);
            // Producer is a child leaf; look up the _din range on the
            // child RTL via get_port_of normalization.
            let data_range = crate::upper_wires::infer_fifo_data_range(
                fifo_name,
                fifo,
                slot_task,
                leaf_modules,
                false,
            );
            submodules.push(build_fifo_instance(
                fifo_name,
                data_range.as_ref(),
                depth,
                Some(&fp_region),
                false,
            ));
        }
    }

    AnyModuleDefinition::new_grouped(
        slot_name.to_owned(),
        ports,
        submodules,
        wires,
    )
}

/// S-AXI control port names (AXI-Lite interface to host).
const S_AXI_CTRL_PORTS: &[&str] = &[
    "AWVALID", "AWREADY", "AWADDR", "WVALID", "WREADY", "WDATA", "WSTRB",
    "ARVALID", "ARREADY", "ARADDR", "RVALID", "RREADY", "RDATA", "RRESP",
    "BVALID", "BREADY", "BRESP",
];

/// Returns true if the named AXI-Lite control port is an input on the
/// slave (top-level) side: master→slave address/data/valid channels and
/// the response-channel READY ports.
fn is_s_axi_slave_input(axi_port: &str) -> bool {
    matches!(
        axi_port,
        "AWVALID" | "AWADDR" | "WVALID" | "WDATA" | "WSTRB"
        | "ARVALID" | "ARADDR" | "BREADY" | "RREADY"
    )
}

/// Port mapping from `ctrl_s_axi` internal name → top-level expression.
/// Python's `_CTRL_S_AXI_PORT_MAPPING` in `gen_rs_graphir.py`.
fn ctrl_s_axi_port_expr(port_name: &str) -> Expression {
    match port_name {
        "ACLK" => Expression::new_id("ap_clk"),
        // Python routes ctrl_s_axi.ARESET through `rst` (output of
        // reset_inverter), same as reset_inverter_0.rst → `rst`.
        "ARESET" => Expression::new_id("rst"),
        "ACLK_EN" => Expression::new_lit("1'b1"),
        _ => {
            // AXI-Lite ports map to s_axi_control_{name} at top level
            if S_AXI_CTRL_PORTS.contains(&port_name) {
                Expression::new_id(&format!("s_axi_control_{port_name}"))
            } else {
                // Control/scalar ports (ap_start, ap_done, etc.) connect to internal wires
                Expression::new_id(port_name)
            }
        }
    }
}

/// Build the top-level module definition with slot instances, FSM, and `ctrl_s_axi`.
#[allow(clippy::too_many_lines, reason = "sequential top-module assembly")]
#[allow(clippy::too_many_arguments, reason = "top-module assembly orchestrator")]
fn build_top_module(
    program: &Program,
    top: &tapa_topology::task::TaskDesign,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
    slot_defs: &[AnyModuleDefinition],
    fsm_modules: &BTreeMap<String, AnyModuleDefinition>,
    fsm_name: &str,
    has_ctrl_s_axi: bool,
    top_rtl_params: &[tapa_graphir::ModuleParameter],
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
) -> AnyModuleDefinition {
    // Default region for top-level system instances (top FSM, ctrl_s_axi,
    // reset_inverter). Python's get_top_module_definition uses the first
    // value of slot_task_name_to_fp_region as default_region; we do the same.
    let default_region = program
        .slot_task_name_to_fp_region
        .as_ref()
        .and_then(|m| m.values().next().cloned());
    let mut ports = vec![
        input_wire("ap_clk", None),
        input_wire("ap_rst_n", None),
    ];

    // Add s_axi_control_* ports (AXI-Lite slave interface) if ctrl_s_axi is
    // present. Directions match the ctrl_s_axi module's own port list:
    // master→slave ports are inputs (AW/W/AR VALID+ADDR+DATA+STRB, BREADY,
    // RREADY); slave→master ports are outputs (AW/W/AR READY, R VALID+DATA+
    // RESP, B VALID+RESP).
    if has_ctrl_s_axi {
        for &axi_port in S_AXI_CTRL_PORTS {
            let port_name = format!("s_axi_control_{axi_port}");
            if is_s_axi_slave_input(axi_port) {
                ports.push(input_wire(&port_name, None));
            } else {
                ports.push(crate::utils::output_wire(&port_name, None));
            }
        }
    }

    // Expand ALL top-level data ports to RTL-level signals
    for port in &top.ports {
        for expanded in crate::utils::expand_port_to_signals(&port.name, port.cat, port.width) {
            if !ports.iter().any(|p| p.name == expanded.name) {
                ports.push(expanded);
            }
        }
    }

    let mut submodules = vec![get_reset_inverter_inst(default_region.as_deref())];
    // `rst` is the output of reset_inverter; `ap_rst` is the same signal
    // under the Vitis-generated name. Python names the wire `rst`; for
    // compatibility we emit both `ap_rst` (in-flight Rust code uses it)
    // and `rst` (Python-equivalent name for ctrl_s_axi's ARESET and the
    // reset_inverter.rst connection).
    let mut wires = vec![
        make_wire("ap_rst", None),
        make_wire("rst", None),
        make_wire("ap_start", None),
        make_wire("ap_done", None),
        make_wire("ap_idle", None),
        make_wire("ap_ready", None),
        make_wire("interrupt", None),
    ];

    // FSM instance — self-connect every port in the FSM module definition
    // the way Python's `_make_fsm_inst` does (each port expression is just
    // an identifier for the same name). For any FSM port that isn't
    // already declared as a top-level port or wire (e.g. slot-prefixed
    // handshake signals like `SLOT_X0Y2_SLOT_X0Y2_0__ap_start`), emit a
    // matching wire so the exporter's DRC can find every identifier.
    let top_fsm_connections: Vec<tapa_graphir::ModuleConnection> =
        if let Some(fsm_def) = fsm_modules.get(fsm_name) {
            for p in fsm_def.ports() {
                let name = &p.name;
                let already_declared = ports.iter().any(|port| port.name == *name)
                    || wires.iter().any(|w| w.name == *name);
                if !already_declared {
                    wires.push(make_wire(name, p.range.clone()));
                }
            }
            fsm_def
                .ports()
                .iter()
                .map(|p| crate::utils::make_connection(&p.name, Expression::new_id(&p.name)))
                .collect()
        } else {
            ["ap_clk", "ap_rst_n", "ap_start", "ap_done", "ap_idle", "ap_ready"]
                .iter()
                .map(|&name| crate::utils::make_connection(name, Expression::new_id(name)))
                .collect()
        };
    submodules.push(tapa_graphir::ModuleInstantiation {
        name: format!("{fsm_name}_0"),
        hierarchical_name: HierarchicalName::get_name(&format!("{fsm_name}_0")),
        module: fsm_name.to_owned(),
        connections: top_fsm_connections,
        parameters: Vec::new(),
        floorplan_region: default_region.clone(),
        area: None,
        pragmas: Vec::new(),
        extra: BTreeMap::default(),
    });

    // ctrl_s_axi instance — maps AXI-Lite ports through s_axi_control_* top ports
    if has_ctrl_s_axi {
        let mut ctrl_connections = Vec::new();
        // Fixed port mappings (clock, reset, enable) + AXI-Lite channel ports.
        // `ctrl_s_axi_port_expr` routes ACLK/ARESET/ACLK_EN and the
        // S_AXI_CTRL_PORTS set; each gets mapped to its top-level wire.
        for &axi_port in ["ACLK", "ARESET", "ACLK_EN"].iter().chain(S_AXI_CTRL_PORTS) {
            ctrl_connections.push(crate::utils::make_connection(
                axi_port,
                ctrl_s_axi_port_expr(axi_port),
            ));
        }
        // Control signal wires (internal, between FSM and ctrl_s_axi)
        for &sig in &["ap_start", "ap_done", "ap_idle", "ap_ready", "interrupt"] {
            ctrl_connections.push(crate::utils::make_connection(sig, Expression::new_id(sig)));
        }
        // Dynamic scalar/MMAP-offset ports — connect to same-name top-level wires
        // Python: _CTRL_S_AXI_PORT_MAPPING defaults unknown ports to Token.new_id(port.name)
        for port in &top.ports {
            use tapa_task_graph::port::ArgCategory;
            let ctrl_port_name = match port.cat {
                ArgCategory::Scalar => port.name.clone(),
                ArgCategory::Mmap | ArgCategory::AsyncMmap
                | ArgCategory::Immap | ArgCategory::Ommap => format!("{}_offset", port.name),
                ArgCategory::Istream | ArgCategory::Ostream
                | ArgCategory::Istreams | ArgCategory::Ostreams => continue,
            };
            ctrl_connections.push(crate::utils::make_connection(
                &ctrl_port_name,
                Expression::new_id(&ctrl_port_name),
            ));
            // Ensure the top module has a wire for this port
            if !wires.iter().any(|w| w.name == ctrl_port_name) {
                wires.push(make_wire(&ctrl_port_name, Some(range_msb(63))));
            }
        }

        // Python's get_top_ctrl_s_axi_inst (gen_rs_graphir.py) passes two
        // parameter assignments: the ctrl_s_axi module exposes
        // C_S_AXI_ADDR_WIDTH / C_S_AXI_DATA_WIDTH, which the top
        // instantiation ties to the top task's
        // C_S_AXI_CONTROL_ADDR_WIDTH / C_S_AXI_CONTROL_DATA_WIDTH.
        // Python copies `Expression(top_param_by_name[value].expr.root)`,
        // i.e., it substitutes the literal token stream of the outer
        // parameter's default expression (e.g., `6`, `32`) rather than
        // referencing the outer parameter by name.
        let ctrl_param_map = [
            ("C_S_AXI_ADDR_WIDTH", "C_S_AXI_CONTROL_ADDR_WIDTH"),
            ("C_S_AXI_DATA_WIDTH", "C_S_AXI_CONTROL_DATA_WIDTH"),
        ];
        let top_param_by_name: std::collections::BTreeMap<&str, &Expression> = top_rtl_params
            .iter()
            .map(|p| (p.name.as_str(), &p.expr))
            .collect();
        let ctrl_parameters: Vec<tapa_graphir::ModuleConnection> = ctrl_param_map
            .iter()
            .map(|(inner, outer)| {
                let expr = top_param_by_name
                    .get(outer)
                    .map_or_else(|| Expression::new_id(outer), |e| (*e).clone());
                tapa_graphir::ModuleConnection {
                    name: (*inner).to_owned(),
                    hierarchical_name: HierarchicalName::get_name(inner),
                    expr,
                    extra: BTreeMap::default(),
                }
            })
            .collect();
        submodules.push(tapa_graphir::ModuleInstantiation {
            name: "control_s_axi_U".into(),
            hierarchical_name: HierarchicalName::get_name("control_s_axi_U"),
            module: format!("{}_control_s_axi", program.top),
            connections: ctrl_connections,
            parameters: ctrl_parameters,
            floorplan_region: default_region,
            area: None,
            pragmas: Vec::new(),
            extra: BTreeMap::default(),
        });
    }

    // Slot instances — Python-equivalent `get_top_level_slot_inst`:
    // build connections by walking each slot's args in the TOP task and
    // running them through the same `_connect_scalar` / `_connect_istream`
    // / `_connect_ostream` / `_connect_mmap` flow as child instances.
    // Scalars and mmap offsets route through the TOP task's arg-table
    // queue-tail wires (`{slot_inst}___{arg}[_offset]__q0`), streams
    // through the slot's own boundary port names, and mmap AXI channels
    // through the parent-visible `m_axi_{arg}_*` wire names.
    let top_arg_table = crate::instantiation_builder::build_arg_table(top);
    for slot_name in slot_to_instances.keys() {
        let slot_def = slot_defs.iter().find(|d| d.name() == slot_name);
        let slot_port_names: Option<std::collections::HashSet<String>> = slot_def
            .map(|d| d.ports().iter().map(|p| p.name.clone()).collect());
        let slot_inst_name = format!("{slot_name}_0");

        // Find the SLOT's args in the top task. Slot tasks are
        // instantiated under top.tasks[slot_name] with a single instance
        // whose args bind top-level scalar / mmap / stream wires to the
        // slot boundary ports.
        let inst_arg_table = top_arg_table.get(&slot_inst_name);
        let mut slot_connections: Vec<tapa_graphir::ModuleConnection> = Vec::new();
        let has_slot_hierarchy = top.tasks.contains_key(slot_name);
        if let Some(slot_task_inst) = top.tasks.get(slot_name).and_then(|v| v.first()) {
            for (port_name, arg) in &slot_task_inst.args {
                let conns = crate::instantiation_builder::build_port_connections(
                    port_name,
                    arg,
                    inst_arg_table,
                    slot_port_names.as_ref(),
                    None,
                );
                slot_connections.extend(conns);
            }
        }
        // Append clock/reset; the four ap_* control connections use
        // per-instance wires when the top task has a slot hierarchy
        // registered (matches Python's `get_top_level_slot_inst`), or
        // the top-level wires otherwise (for trivial fixtures that
        // synthesize "slot" names from floorplan regions rather than
        // from task names).
        slot_connections.push(crate::utils::make_connection(
            "ap_clk",
            Expression::new_id("ap_clk"),
        ));
        slot_connections.push(crate::utils::make_connection(
            "ap_rst_n",
            Expression::new_id("ap_rst_n"),
        ));
        for sig in &["ap_start", "ap_done", "ap_ready", "ap_idle"] {
            let expr = if has_slot_hierarchy {
                Expression::new_id(&format!("{slot_inst_name}__{sig}"))
            } else {
                Expression::new_id(sig)
            };
            slot_connections.push(crate::utils::make_connection(sig, expr));
        }

        let slot_fp_region = program
            .slot_task_name_to_fp_region
            .as_ref()
            .and_then(|m| m.get(slot_name).cloned())
            .unwrap_or_else(|| slot_name.clone());
        submodules.push(tapa_graphir::ModuleInstantiation {
            name: slot_inst_name.clone(),
            hierarchical_name: HierarchicalName::get_name(&slot_inst_name),
            module: slot_name.clone(),
            connections: slot_connections,
            parameters: Vec::new(),
            floorplan_region: Some(slot_fp_region),
            area: None,
            pragmas: Vec::new(),
            extra: BTreeMap::default(),
        });
    }

    // Top-level FIFO instances: FIFOs whose producer and consumer are in
    // different slots become top-level submodules. Python's
    // `get_top_ir_subinsts` adds one `fifo` instance per such FIFO,
    // assigned to the consumer's slot region. Matching Python here closes
    // the submodule-count parity gap on the shared fixture.
    if let Some(region_map) = program.slot_task_name_to_fp_region.as_ref() {
        for (fifo_name, fifo) in &top.fifos {
            let consumer_slot = fifo.consumed_by.as_ref().map(|e| &e.0);
            let producer_slot = fifo.produced_by.as_ref().map(|e| &e.0);
            let cross_slot = matches!(
                (consumer_slot, producer_slot),
                (Some(c), Some(p)) if c != p
            );
            if !cross_slot {
                continue;
            }
            // Ensure the top module declares the data/handshake wires
            // every cross-slot FIFO needs for its connections. Without
            // these the exporter's DRC fails because the FIFO instance
            // references undeclared identifiers.
            for suffix in ["_din", "_dout", "_empty_n", "_full_n", "_read", "_write"] {
                let wire_name = format!("{fifo_name}{suffix}");
                if !ports.iter().any(|p| p.name == wire_name)
                    && !wires.iter().any(|w| w.name == wire_name)
                {
                    wires.push(make_wire(&wire_name, None));
                }
            }
            let depth = fifo.depth.unwrap_or(32);
            // Cross-slot FIFO: drill into the producer slot's child leaf
            // RTL to get the `_din` port range (Python looks up the
            // slot-def port, but at this point the slot defs have not
            // yet been rewritten with Python-equivalent ports).
            let data_range = crate::upper_wires::infer_top_fifo_data_range_via_leaf(
                fifo_name,
                fifo,
                program,
                leaf_modules,
            );
            // Region: use the consumer's slot region (matches Python's
            // `floorplan_task_name_region_mapping[fifo['consumed_by'][0]]`).
            let fifo_region = consumer_slot
                .and_then(|c| region_map.get(c).cloned())
                .or_else(|| region_map.values().next().cloned());
            let fifo_inst = build_fifo_instance(
                fifo_name,
                data_range.as_ref(),
                depth,
                fifo_region.as_deref(),
                true,
            );
            submodules.push(fifo_inst);
        }
    }

    AnyModuleDefinition::new_grouped(
        program.top.clone(),
        ports,
        submodules,
        wires,
    )
}

/// Find the parent-visible arg name for a child port in a given task.
fn find_arg_name_in_task(
    parent: &tapa_topology::task::TaskDesign,
    task_name: &str,
    inst_name: &str,
    port_name: &str,
) -> Option<String> {
    let instances = parent.tasks.get(task_name)?;
    for (idx, inst) in instances.iter().enumerate() {
        if format!("{task_name}_{idx}") == inst_name {
            for (child_port, arg) in &inst.args {
                if child_port == port_name {
                    return Some(arg.arg.clone());
                }
            }
        }
    }
    None
}

/// Find the grouped module named `name` and return mutable references to its
/// base + grouped fields. Centralizes the `AnyModuleDefinition::Grouped`
/// pattern used by several post-pass rewrites in `build_project_from_state`.
fn find_grouped_mut<'a>(
    defs: &'a mut [AnyModuleDefinition],
    name: &str,
) -> Option<(&'a mut BaseFields, &'a mut GroupedFields)> {
    for module in defs {
        if let AnyModuleDefinition::Grouped { base, grouped, .. } = module {
            if base.name == name {
                return Some((base, grouped));
            }
        }
    }
    None
}

/// Parse `taskname_idx` into `(task_name, idx)`.
fn parse_instance_name(name: &str) -> (String, u32) {
    if let Some(last_underscore) = name.rfind('_') {
        if let Ok(idx) = name[last_underscore + 1..].parse::<u32>() {
            return (name[..last_underscore].to_owned(), idx);
        }
    }
    (name.to_owned(), 0)
}

/// Build a port range `[width-1:0]` if width > 1.
fn port_range(width: u32) -> Option<tapa_graphir::Range> {
    if width > 1 {
        Some(range_msb(width - 1))
    } else {
        None
    }
}

/// Build interfaces for all module definitions.
///
/// Python-equivalent interface assembly: dedicated builders for FIFO,
/// `reset_inverter`, FSM, `ctrl_s_axi`, slot/top tasks. Each builder
/// produces the correct interface types with valid/ready ports.
#[allow(clippy::too_many_lines, reason = "sequential interface assembly per module type")]
fn build_interfaces(
    module_defs: &[AnyModuleDefinition],
    program: &Program,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<AnyInterface>> {
    use tapa_graphir::interface::InterfaceBase;
    let mut ifaces = BTreeMap::new();

    let make_hs = |ports: Vec<String>, clk: Option<&str>, rst: Option<&str>,
                   valid: &str, ready: &str| -> AnyInterface {
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
            // Python: get_fifo_ifaces()
            vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["clk".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()), rst_port: None,
                        ports: vec!["clk".into(), "reset".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                make_hs(
                    vec!["if_din".into(), "if_full_n".into(), "if_write".into(), "clk".into(), "reset".into()],
                    Some("clk"), Some("reset"), "if_write", "if_full_n",
                ),
                make_hs(
                    vec!["if_dout".into(), "if_empty_n".into(), "if_read".into(), "clk".into(), "reset".into()],
                    Some("clk"), Some("reset"), "if_empty_n", "if_read",
                ),
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["if_read_ce".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["if_write_ce".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ]
        } else if name == "reset_inverter" {
            // Python: get_reset_inverter_ifaces()
            vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["clk".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()), rst_port: None,
                        ports: vec!["clk".into(), "rst_n".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("clk".into()), rst_port: None,
                        ports: vec!["clk".into(), "rst".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ]
        } else if name.ends_with("_control_s_axi") {
            // Python: get_ctrl_s_axi_ifaces()
            let mut ci = vec![
                AnyInterface::Clock {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["ACLK".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FeedForwardReset {
                    base: InterfaceBase {
                        clk_port: Some("ACLK".into()), rst_port: None,
                        ports: vec!["ACLK".into(), "ARESET".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
                AnyInterface::FalsePath {
                    base: InterfaceBase {
                        clk_port: None, rst_port: None,
                        ports: vec!["ACLK_EN".into()],
                        role: String::new(), origin_info: String::new(),
                    },
                    extra: BTreeMap::default(),
                },
            ];
            // 5 AXI-Lite channel handshakes
            for (ports_list, valid, ready) in [
                (vec!["ARADDR", "ARREADY", "ARVALID"], "ARVALID", "ARREADY"),
                (vec!["AWADDR", "AWREADY", "AWVALID"], "AWVALID", "AWREADY"),
                (vec!["BREADY", "BRESP", "BVALID"], "BVALID", "BREADY"),
                (vec!["RDATA", "RREADY", "RRESP", "RVALID"], "RVALID", "RREADY"),
                (vec!["WDATA", "WREADY", "WSTRB", "WVALID"], "WVALID", "WREADY"),
            ] {
                ci.push(make_hs(
                    ports_list.iter().map(|&s| s.to_owned()).collect(),
                    Some("ACLK"), Some("ARESET"), valid, ready,
                ));
            }
            ci.push(AnyInterface::FeedForward {
                base: InterfaceBase {
                    clk_port: Some("ACLK".into()), rst_port: Some("ARESET".into()),
                    ports: vec!["ACLK".into(), "ARESET".into(), "interrupt".into()],
                    role: String::new(), origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            // ApCtrl with scalar ports + control
            let mut ap_ports: Vec<String> = def.ports().iter()
                .map(|p| p.name.clone())
                .filter(|n| !CTRL_S_AXI_FIXED_PORTS.contains(&n.as_str()))
                .collect();
            ap_ports.extend(["ACLK", "ARESET", "ap_start", "ap_done", "ap_ready", "ap_idle"]
                .iter().map(|&s| s.to_owned()));
            ci.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some("ACLK".into()), rst_port: Some("ARESET".into()),
                    ports: ap_ports, role: String::new(), origin_info: String::new(),
                },
                ap_start_port: Some("ap_start".into()),
                ap_done_port: Some("ap_done".into()),
                ap_ready_port: Some("ap_ready".into()),
                ap_idle_port: Some("ap_idle".into()),
                ap_continue_port: None,
                extra: BTreeMap::default(),
            });
            ci
        } else if name == program.top {
            // Top task: stream/MMAP interfaces + optional s_axi_control
            // channel handshakes (only if the top module actually exposes
            // the `s_axi_control_*` ports — i.e. ctrl_s_axi is present).
            let mut ti = build_task_port_ifaces(def, &port_names);
            let has_s_axi_ports = port_names.contains("s_axi_control_ARVALID");
            if has_s_axi_ports {
                for (channel, valid, ready) in [
                    (&["ARADDR", "ARREADY", "ARVALID"][..], "ARVALID", "ARREADY"),
                    (&["AWADDR", "AWREADY", "AWVALID"][..], "AWVALID", "AWREADY"),
                    (&["BREADY", "BRESP", "BVALID"][..], "BVALID", "BREADY"),
                    (&["RDATA", "RREADY", "RRESP", "RVALID"][..], "RVALID", "RREADY"),
                    (&["WDATA", "WREADY", "WSTRB", "WVALID"][..], "WVALID", "WREADY"),
                ] {
                    let mut ports: Vec<String> = channel
                        .iter()
                        .map(|&s| format!("s_axi_control_{s}"))
                        .collect();
                    ports.extend(["ap_clk".into(), "ap_rst_n".into()]);
                    ti.push(make_hs(
                        ports,
                        Some("ap_clk"),
                        Some("ap_rst_n"),
                        &format!("s_axi_control_{valid}"),
                        &format!("s_axi_control_{ready}"),
                    ));
                }
            }
            ti
        } else if slot_to_instances.contains_key(name) {
            // Slot task: stream/MMAP interfaces + ApCtrl + Clock + FeedForwardReset
            let mut si = Vec::new();
            let mut scalars = Vec::new();
            build_task_port_ifaces_with_scalars(def, &port_names, &mut si, &mut scalars);
            // Python: get_slot_task_ifaces(scalars)
            let mut ap_ports = scalars;
            ap_ports.extend(["ap_clk", "ap_rst_n", "ap_start", "ap_done", "ap_ready", "ap_idle"]
                .iter().map(|&s| s.to_owned()));
            si.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some("ap_clk".into()), rst_port: Some("ap_rst_n".into()),
                    ports: ap_ports, role: String::new(), origin_info: String::new(),
                },
                ap_start_port: Some("ap_start".into()),
                ap_done_port: Some("ap_done".into()),
                ap_ready_port: Some("ap_ready".into()),
                ap_idle_port: Some("ap_idle".into()),
                ap_continue_port: None,
                extra: BTreeMap::default(),
            });
            si.push(AnyInterface::Clock {
                base: InterfaceBase {
                    clk_port: None, rst_port: None,
                    ports: vec!["ap_clk".into()],
                    role: String::new(), origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            si.push(AnyInterface::FeedForwardReset {
                base: InterfaceBase {
                    clk_port: Some("ap_clk".into()), rst_port: None,
                    ports: vec!["ap_clk".into(), "ap_rst_n".into()],
                    role: String::new(), origin_info: String::new(),
                },
                extra: BTreeMap::default(),
            });
            si
        } else if name.ends_with("_fsm") && name == format!("{}_fsm", program.top) {
            // Top-level FSM: emit per-slot ApCtrl interfaces plus the
            // FSM-top ApCtrl (matches Python get_fsm_ifaces).
            let mut fi = Vec::new();
            let slot_names: Vec<String> = slot_to_instances.keys().cloned().collect();

            for slot_name in &slot_names {
                let slot_prefix = format!("{slot_name}_0");
                let start = format!("{slot_prefix}__ap_start");
                let done = format!("{slot_prefix}__ap_done");
                let ready = format!("{slot_prefix}__ap_ready");
                let idle = format!("{slot_prefix}__ap_idle");
                // Only emit a per-slot ApCtrl if the FSM actually exposes the
                // slot-prefixed handshake ports. Python's equivalent always
                // emits it; we guard here so that minimal test fixtures
                // without slot instantiations still produce a valid project.
                if !port_names.contains(&start) || !port_names.contains(&done) {
                    continue;
                }
                let mut ap_ports: Vec<String> = vec!["ap_clk".into(), "ap_rst_n".into()];
                ap_ports.extend(
                    def.ports()
                        .iter()
                        .filter(|p| p.name.starts_with(&slot_prefix))
                        .map(|p| p.name.clone()),
                );
                fi.push(AnyInterface::ApCtrl {
                    base: InterfaceBase {
                        clk_port: Some("ap_clk".into()),
                        rst_port: Some("ap_rst_n".into()),
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
            // Matches Python `get_fsm_ifaces`, which emits this interface
            // unconditionally on the top FSM module. Role inference then
            // validates the directions.
            let fsm_scalars: Vec<String> = def
                .ports()
                .iter()
                .map(|p| p.name.clone())
                .filter(|pn| {
                    pn != "ap_clk"
                        && pn != "ap_rst_n"
                        && !slot_names
                            .iter()
                            .any(|slot| pn.starts_with(&format!("{slot}_0")))
                })
                .collect();
            let mut top_ap_ports = fsm_scalars;
            top_ap_ports.extend(
                ["ap_clk", "ap_rst_n", "ap_start", "ap_done", "ap_ready", "ap_idle"]
                    .iter()
                    .map(|&s| s.to_owned()),
            );
            fi.push(AnyInterface::ApCtrl {
                base: InterfaceBase {
                    clk_port: Some("ap_clk".into()),
                    rst_port: Some("ap_rst_n".into()),
                    ports: top_ap_ports,
                    role: String::new(),
                    origin_info: String::new(),
                },
                ap_start_port: Some("ap_start".into()),
                ap_done_port: Some("ap_done".into()),
                ap_ready_port: Some("ap_ready".into()),
                ap_idle_port: Some("ap_idle".into()),
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

/// Fixed ports for `ctrl_s_axi` module (excluded from scalar interface).
const CTRL_S_AXI_FIXED_PORTS: &[&str] = &[
    "ACLK", "ACLK_EN", "ARESET", "interrupt",
    "ARADDR", "ARREADY", "ARVALID", "AWADDR", "AWREADY", "AWVALID",
    "BREADY", "BRESP", "BVALID", "RDATA", "RREADY", "RRESP", "RVALID",
    "WDATA", "WREADY", "WSTRB", "WVALID",
    "ap_start", "ap_done", "ap_ready", "ap_idle",
];

/// Build stream and MMAP handshake interfaces for a task module's ports.
fn build_task_port_ifaces(
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
/// Matches Python's `_append_task_port_ifaces` + `_append_stream_iface`
/// + `_append_mmap_ifaces`.
fn build_task_port_ifaces_with_scalars(
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
        let is_istream = port.name.ends_with("_dout") || port.name.ends_with("_empty_n")
            || port.name.ends_with("_read");
        let is_output_stream = port.name.ends_with("_din") || port.name.ends_with("_full_n")
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
                (&["_din", "_full_n", "_write"], "_write", "_full_n")
            } else {
                (&["_dout", "_empty_n", "_read"], "_empty_n", "_read")
            };
            let mut ps: Vec<String> = suffixes.iter().map(|s| format!("{base}{s}")).collect();
            ps.extend(["ap_clk".into(), "ap_rst_n".into()]);
            ifaces.push(AnyInterface::HandShake {
                base: InterfaceBase {
                    clk_port: Some("ap_clk".into()), rst_port: Some("ap_rst_n".into()),
                    ports: ps, role: String::new(), origin_info: String::new(),
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
    let mut mmap_bases = std::collections::HashSet::new();
    for port in ports {
        if port.name.ends_with("_offset") && !port.name.starts_with("m_axi_") {
            let base = port.name.trim_end_matches("_offset");
            mmap_bases.insert(base.to_owned());
        }
    }
    for base in &mmap_bases {
        scalars.push(format!("{base}_offset"));
        // Per-channel MMAP handshakes
        for (channel_ports, valid_suffix, ready_suffix) in [
            (&["_ARVALID", "_ARREADY", "_ARADDR", "_ARID", "_ARLEN", "_ARSIZE", "_ARBURST", "_ARLOCK", "_ARCACHE", "_ARPROT", "_ARQOS", "_ARREGION"][..], "_ARVALID", "_ARREADY"),
            (&["_RVALID", "_RREADY", "_RDATA", "_RLAST", "_RID", "_RRESP"][..], "_RVALID", "_RREADY"),
            (&["_AWVALID", "_AWREADY", "_AWADDR", "_AWID", "_AWLEN", "_AWSIZE", "_AWBURST", "_AWLOCK", "_AWCACHE", "_AWPROT", "_AWQOS", "_AWREGION"][..], "_AWVALID", "_AWREADY"),
            (&["_WVALID", "_WREADY", "_WDATA", "_WSTRB", "_WLAST"][..], "_WVALID", "_WREADY"),
            (&["_BVALID", "_BREADY", "_BID", "_BRESP"][..], "_BVALID", "_BREADY"),
        ] {
            let valid_port = format!("m_axi_{base}{valid_suffix}");
            let ready_port = format!("m_axi_{base}{ready_suffix}");
            if !port_names.contains(&valid_port) || !port_names.contains(&ready_port) {
                continue;
            }
            let mut ch_ports: Vec<String> = channel_ports.iter()
                .map(|s| format!("m_axi_{base}{s}"))
                .filter(|n| port_names.contains(n))
                .collect();
            ch_ports.extend(["ap_clk".into(), "ap_rst_n".into()]);
            ifaces.push(AnyInterface::HandShake {
                base: InterfaceBase {
                    clk_port: Some("ap_clk".into()), rst_port: Some("ap_rst_n".into()),
                    ports: ch_ports, role: String::new(), origin_info: String::new(),
                },
                valid_port: Some(valid_port),
                ready_port: Some(ready_port),
                data_ports: Vec::new(),
                extra: BTreeMap::default(),
            });
        }
    }
}

/// Extract stream base name from a suffixed port name.
fn extract_stream_base(name: &str) -> Option<&str> {
    for suffix in &["_dout", "_empty_n", "_read", "_din", "_full_n", "_write"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return Some(base);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_program() -> Program {
        serde_json::from_str(
            r#"{
                "top": "top_task",
                "target": "xilinx-hls",
                "tasks": {
                    "top_task": {
                        "level": "upper", "code": "", "target": "xilinx-hls",
                        "ports": [
                            {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                        ],
                        "tasks": {
                            "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                        },
                        "fifos": {}
                    },
                    "child": {
                        "level": "lower", "code": "", "target": "xilinx-hls",
                        "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                        "tasks": {}, "fifos": {}
                    }
                }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn build_project_produces_modules() {
        let prog = make_program();
        let leaf_mods = BTreeMap::from([(
            "child".into(),
            AnyModuleDefinition::new_verilog(
                "child".into(),
                Vec::new(),
                "module child(); endmodule".into(),
            ),
        )]);
        let fsm_mods = BTreeMap::new();
        let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

        let project = build_project(&prog, &leaf_mods, &fsm_mods, None, &slot_to_insts, None, None, None).unwrap();

        assert!(project.has_module("top_task"), "should have top module");
        assert!(project.has_module("SLOT_0"), "should have slot module");
        assert!(project.has_module("child"), "should have leaf module");
        assert!(project.has_module("fifo"), "should have fifo template");
        assert!(project.has_module("reset_inverter"), "should have reset_inverter");
    }

    #[test]
    fn parse_instance_name_works() {
        assert_eq!(parse_instance_name("producer_0"), ("producer".into(), 0));
        assert_eq!(parse_instance_name("child_a_2"), ("child_a".into(), 2));
        assert_eq!(parse_instance_name("single"), ("single".into(), 0));
    }

    #[test]
    fn build_project_missing_top_task() {
        let prog: Program = serde_json::from_str(r#"{
            "top": "nonexistent",
            "target": "xilinx-hls",
            "tasks": {}
        }"#).unwrap();
        let result = build_project(&prog, &BTreeMap::new(), &BTreeMap::new(), None, &BTreeMap::new(), None, None, None);
        assert!(result.is_err(), "should fail for missing top task");
    }

    #[test]
    fn build_project_applies_interface_roles() {
        let prog = make_program();
        let leaf_mods = BTreeMap::from([(
            "child".into(),
            AnyModuleDefinition::new_verilog(
                "child".into(),
                Vec::new(),
                "module child(); endmodule".into(),
            ),
        )]);
        let fsm_mods = BTreeMap::new();
        let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

        let project =
            build_project(&prog, &leaf_mods, &fsm_mods, None, &slot_to_insts, None, None, None)
                .expect("build succeeded");
        let ifaces = project.ifaces.as_ref().expect("project has interfaces");
        // The FIFO module has handshake interfaces — roles must be source/sink
        // after inference, never the default.
        let fifo_ifaces = ifaces
            .get("fifo")
            .expect("fifo module must have interfaces attached");
        let roles: std::collections::HashSet<_> =
            fifo_ifaces.iter().map(|i| i.base().role.clone()).collect();
        assert!(
            roles.contains("source") || roles.contains("sink"),
            "fifo must have at least one source or sink role, got {roles:?}"
        );
    }

    #[test]
    fn build_project_rejects_invalid_interface_direction() {
        let prog = make_program();
        let leaf_mods = BTreeMap::new();
        let fsm_mods = BTreeMap::new();
        // No ctrl_s_axi → no s_axi_control_* ports → no top handshakes.
        let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

        // Build the project normally, then deliberately corrupt the fifo's
        // if_write port direction so role inference fails.
        let mut project =
            build_project(&prog, &leaf_mods, &fsm_mods, None, &slot_to_insts, None, None, None)
                .expect("build succeeded");
        for def in &mut project.modules.module_definitions {
            if let AnyModuleDefinition::Verilog { base, .. } = def {
                if base.name == "fifo" {
                    for port in &mut base.ports {
                        if port.name == "if_write" {
                            port.port_type = "output wire".into();
                        }
                    }
                }
            }
        }
        // Re-run role inference — should now fail.
        let mut ifaces = project.ifaces.clone().unwrap_or_default();
        let defs = project.modules.module_definitions.clone();
        let err = crate::iface_roles::apply_iface_roles(&defs, &mut ifaces)
            .expect_err("corrupted fifo must fail role inference");
        assert!(
            matches!(err, LoweringError::InterfaceDirection(_)),
            "expected InterfaceDirection error, got: {err:?}"
        );
    }

    #[test]
    fn aggregate_slot_params_matches_python_alphabetical_order() {
        // Slot-parameter aggregation must iterate child tasks
        // alphabetically to match Python's `dict(sorted(tasks.items()))`
        // in `tapa/task.py`. The JSON lists `zleaf` first and `aleaf`
        // second, but Python sorts them so `aleaf` wins as the
        // first-seen parameter source. Verified against Python: for
        // this exact JSON, `task.instances == ['aleaf_0', 'zleaf_0']`
        // and the aggregated `P = 3'd1` (from `aleaf`). Rust must
        // produce the same.
        let prog_json = r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "slot_A": [{"args": {}}]
                    },
                    "fifos": {}
                },
                "slot_A": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "is_slot": true,
                    "ports": [],
                    "tasks": {
                        "zleaf": [{"args": {}}],
                        "aleaf": [{"args": {}}]
                    },
                    "fifos": {}
                },
                "zleaf": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [], "tasks": {}, "fifos": {}
                },
                "aleaf": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [], "tasks": {}, "fifos": {}
                }
            }
        }"#;
        let prog: Program = serde_json::from_str(prog_json).unwrap();
        // Slot's children iterate alphabetically — `aleaf` before
        // `zleaf` — matching Python's `dict(sorted(...))` semantics.
        let slot_a = prog.tasks.get("slot_A").unwrap();
        let keys: Vec<&String> = slot_a.tasks.keys().collect();
        assert_eq!(
            keys,
            vec!["aleaf", "zleaf"],
            "BTreeMap must iterate alphabetically (Python-equivalent)"
        );

        // Build a minimal TopologyWithRtl where each leaf carries a
        // distinct expression for parameter `P`. aggregate_slot_leaf_parameters
        // should pick the first-seen (alphabetical = `aleaf`) expression.
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(prog);
        let make_rtl = |mod_name: &str, p_val: &str| -> tapa_rtl::VerilogModule {
            let src = format!(
                "module {mod_name} #(parameter P = {p_val}) (); endmodule\n"
            );
            tapa_rtl::VerilogModule::parse(&src).unwrap()
        };
        state
            .attach_module("zleaf", make_rtl("zleaf", "10'd1"))
            .expect("attach zleaf");
        state
            .attach_module("aleaf", make_rtl("aleaf", "3'd1"))
            .expect("attach aleaf");

        let slot_to_insts = BTreeMap::from([("slot_A".into(), vec!["aleaf_0".into(), "zleaf_0".into()])]);
        let fsm_mods = BTreeMap::new();
        let leaf_mods = BTreeMap::from([
            ("zleaf".into(), AnyModuleDefinition::new_verilog("zleaf".into(), Vec::new(), "module zleaf; endmodule".into())),
            ("aleaf".into(), AnyModuleDefinition::new_verilog("aleaf".into(), Vec::new(), "module aleaf; endmodule".into())),
        ]);

        let project = build_project(
            &state.program,
            &leaf_mods,
            &fsm_mods,
            None,
            &slot_to_insts,
            None,
            None,
            Some(&state),
        )
        .expect("build succeeded");

        // Run the same post-pass aggregation `build_project_from_state`
        // would run — this is where the ordering matters.
        let mut project = project;
        aggregate_slot_leaf_parameters(&mut project, &state, &slot_to_insts);

        let slot_def = project
            .modules
            .module_definitions
            .iter()
            .find(|m| m.name() == "slot_A")
            .expect("slot_A module");
        let AnyModuleDefinition::Grouped { base, .. } = slot_def else {
            panic!("slot_A should be grouped");
        };
        let p_param = base
            .parameters
            .iter()
            .find(|p| p.name == "P")
            .expect("slot_A should carry aggregated parameter P");
        assert_eq!(
            p_param.expr.0.len(),
            1,
            "expected single-token expression, got {:?}",
            p_param.expr.0
        );
        assert_eq!(
            p_param.expr.0[0].repr, "3'd1",
            "alphabetical-first leaf `aleaf` (P = 3'd1) should win, matching Python's \
             dict(sorted(...)) semantics — not `zleaf`'s 10'd1"
        );
    }
}
