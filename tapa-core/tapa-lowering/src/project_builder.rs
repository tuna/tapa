//! Top-level project assembly: builds a `GraphIR` Project from topology + RTL.

use std::collections::BTreeMap;

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_graphir::{AnyModuleDefinition, ModulePort, Modules, Project};
use tapa_topology::program::Program;

use crate::instantiation_builder::{build_arg_table, build_port_connections};
use crate::interfaces::build_interfaces;
use crate::module_defs::{get_fifo_def, get_reset_inverter_def};
use crate::slot_module::build_slot_module;
use crate::top_module::build_top_module;
use crate::utils::{find_grouped_mut, instance_matches_name};
use crate::LoweringError;

/// Build a `GraphIR` Project from a `TopologyWithRtl` state.
///
/// This is the lowest-level RTL-bearing entrypoint. It derives leaf modules
/// and FSM modules from the state, and takes the real `{top}_control_s_axi.v`
/// text as input rather than fabricating a placeholder; the source is
/// parse-validated before it is embedded in the project.
///
/// Callers that want the equivalent path boundary should instead use
/// `build_project_from_paths` via `LoweringInputs`.
#[allow(clippy::too_many_lines, reason = "sequential grouped-module post-pass")]
pub fn build_project_from_state(
    state: &TopologyWithRtl,
    ctrl_s_axi_verilog: &str,
    slot_to_instances: &BTreeMap<String, Vec<String>>,
    island_to_pblock_range: Option<BTreeMap<String, Vec<String>>>,
    part_num: Option<String>,
) -> Result<Project, LoweringError> {
    tapa_rtl::VerilogModule::parse(ctrl_s_axi_verilog).map_err(|e| {
        LoweringError::MissingCtrlSAxi(format!(
            "invalid `{}_control_s_axi` RTL source ({e}); pass the real \
             Verilog via ctrl_s_axi_verilog or use build_project_from_paths",
            state.program.top
        ))
    })?;
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
    // attached RTL. `get_task_graphir_parameters(task_module)` does
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
        let ports: Vec<ModulePort> = mm
            .effective_ports()
            .iter()
            .map(|port| {
                let port_type = match (port.direction, mm.effective_signal_kind(&port.name)) {
                    (tapa_rtl::port::Direction::Input, _) => "input wire",
                    (
                        tapa_rtl::port::Direction::Output,
                        Some(tapa_rtl::signal::SignalKind::Reg),
                    ) => "output reg",
                    (tapa_rtl::port::Direction::Output, _) => "output wire",
                    (tapa_rtl::port::Direction::Inout, _) => "inout wire",
                };
                crate::utils::rtl_port_to_graphir_with_type(port, port_type)
            })
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
    // default to empty parameters; `VecAdd` / `SLOT_*_SLOT_*`
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
    // RTL's own ports, matching `get_task_graphir_ports(top.rtl_module)`
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
    // rewritten, so `get_upper_task_ir_wires(top, slot_defs, ...)`
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
            // identifiers; the full equivalent wire list is
            // installed after the slot rewrite below.
            grouped.wires.retain(|w| !top_port_names.contains(&w.name));
        }
    }

    // Replace slot grouped-module port lists with the equivalent output:
    //   * For each slot port, find a child instance whose arg.arg equals
    //     the slot port name (`_find_port_child`).
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
    // mirroring skip behavior.
    let slot_names: Vec<String> = project
        .modules
        .module_definitions
        .iter()
        .filter_map(|m| {
            if let AnyModuleDefinition::Grouped { base, .. } = m {
                if base.name != state.program.top && state.program.tasks.contains_key(&base.name) {
                    return Some(base.name.clone());
                }
            }
            None
        })
        .collect();
    for slot_name in slot_names {
        let Some(new_ports) = crate::slot_ports::build_slot_ports(&slot_name, state, &leaf_modules)
        else {
            continue;
        };
        let Some(slot_task) = state.program.tasks.get(&slot_name) else {
            continue;
        };
        // equivalent slot wires from `get_upper_task_ir_wires`.
        // We do NOT auto-declare wires for Vitis FSM RTL ports the current
        // wire builder never emits — compatibility with strict
        // `get_upper_task_ir_wires` output is the contract.
        let new_wires = crate::upper_wires::build_upper_task_ir_wires(
            slot_task,
            &new_ports,
            &[],
            &leaf_modules,
        );
        let mut new_wires = new_wires;
        let mut declared: std::collections::BTreeSet<String> =
            new_ports.iter().map(|p| p.name.clone()).collect();
        declared.extend(new_wires.iter().map(|w| w.name.clone()));
        let fsm_name = format!("{slot_name}_fsm");
        if let Some(fsm_def) = project
            .modules
            .module_definitions
            .iter()
            .find(|m| m.name() == fsm_name)
        {
            for port in fsm_def.ports() {
                if declared.insert(port.name.clone()) {
                    new_wires.push(crate::utils::make_wire(&port.name, port.range.clone()));
                }
            }
        }
        if let Some((base, grouped)) =
            find_grouped_mut(&mut project.modules.module_definitions, &slot_name)
        {
            base.ports.clone_from(&new_ports);
            grouped.wires.clone_from(&new_wires);
            let port_names: std::collections::BTreeSet<String> =
                new_ports.iter().map(|p| p.name.clone()).collect();
            crate::slot_ports::declare_missing_connection_wires(grouped, &port_names);
        }
    }

    refresh_top_slot_instance_connections(&mut project.modules.module_definitions, &state.program);

    // Top-wire rewrite: must run AFTER slots are finalized so
    // `build_upper_task_ir_wires` sees the slot grouped defs as the
    // top task's submodule IR defs (passes `slot_defs` here,
    // not leaf defs — top-level FIFOs are produced/consumed by slots).
    let top_name = &state.program.top;
    if state.module_map.contains_key(top_name) {
        if let Some(top_task) = state.program.tasks.get(top_name) {
            // Build a merged `ir_defs` map: slot grouped defs plus leaf
            // defs. The wire builder's FIFO data-range inference walks
            // the producer's IR-def ports; top FIFOs' producers are
            // slots, so including slot defs is what makes the range
            // match current.
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
            new_wires.extend(crate::upper_wires::build_top_extra_wires(&ctrl_s_axi_ports));
            let mut declared: std::collections::BTreeSet<String> =
                top_rtl_ports.iter().map(|p| p.name.clone()).collect();
            declared.extend(new_wires.iter().map(|w| w.name.clone()));
            let fsm_name = format!("{top_name}_fsm");
            if let Some(fsm_def) = project
                .modules
                .module_definitions
                .iter()
                .find(|m| m.name() == fsm_name)
            {
                for port in fsm_def.ports() {
                    if declared.insert(port.name.clone()) {
                        new_wires.push(crate::utils::make_wire(&port.name, port.range.clone()));
                    }
                }
            }
            new_wires.push(crate::utils::make_wire("rst", None));
            if let Some((_, grouped)) =
                find_grouped_mut(&mut project.modules.module_definitions, top_name)
            {
                grouped.wires.clone_from(&new_wires);
                let port_names: std::collections::BTreeSet<String> =
                    top_rtl_ports.iter().map(|p| p.name.clone()).collect();
                crate::slot_ports::declare_missing_connection_wires(grouped, &port_names);
            }
        }
    }

    // Aggregate slot module parameters from each slot's child leaf RTL —
    // matches `get_slot_module_definition_parameters`. For every
    // slot task, walk the child leaf tasks (via the slot's `tasks`
    // dictionary), collect their RTL parameter lists from
    // `state.module_map`, and dedupe by name.
    aggregate_slot_leaf_parameters(&mut project, state, slot_to_instances);

    // Rebuild slot-module interfaces on the finalized slot-port lists.
    // `build_project` runs before `build_slot_ports`,
    // so its slot ifaces reflect the topology-synthesized port list —
    // which includes internal FIFO signals (e.g. `b_q_VecAdd_din` on
    // SLOT_X3Y3) that never exposes on the slot boundary and
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

fn refresh_top_slot_instance_connections(
    module_defs: &mut [AnyModuleDefinition],
    program: &Program,
) {
    let Some(top_task) = program.tasks.get(&program.top) else {
        return;
    };
    let top_arg_table = build_arg_table(top_task);
    let slot_port_names: BTreeMap<String, std::collections::HashSet<String>> = module_defs
        .iter()
        .filter_map(|module| {
            let AnyModuleDefinition::Grouped { base, .. } = module else {
                return None;
            };
            if base.name == program.top || !top_task.tasks.contains_key(&base.name) {
                return None;
            }
            Some((
                base.name.clone(),
                module.ports().iter().map(|p| p.name.clone()).collect(),
            ))
        })
        .collect();

    let Some((_, grouped)) = find_grouped_mut(module_defs, &program.top) else {
        return;
    };

    for inst in &mut grouped.submodules {
        let Some(known_ports) = slot_port_names.get(&inst.module) else {
            continue;
        };
        let Some(slot_instances) = top_task.tasks.get(&inst.module) else {
            continue;
        };
        let Some(slot_task_inst) =
            slot_instances
                .iter()
                .enumerate()
                .find_map(|(idx, candidate)| {
                    instance_matches_name(&inst.module, idx, candidate, &inst.name)
                        .then_some(candidate)
                })
        else {
            continue;
        };
        let arg_table_entry = top_arg_table.get(&inst.name);
        for (port_name, arg) in &slot_task_inst.args {
            let conns =
                build_port_connections(port_name, arg, arg_table_entry, Some(known_ports), None);
            for conn in conns {
                if !inst
                    .connections
                    .iter()
                    .any(|existing| existing.name == conn.name)
                {
                    inst.connections.push(conn);
                }
            }
        }
    }
}

/// Aggregate leaf RTL parameters onto slot grouped modules.
///
/// Iterate every leaf module's parameters in a deterministic order
/// and keep the first-seen `ModuleParameter` for each name verbatim.
/// Child tasks iterate alphabetically by task name (the design task
/// map is sorted), so the alphabetically-first leaf wins as the
/// parameter source; `BTreeMap<String, Vec<InstanceDesign>>`
/// iteration preserves exactly that order.
///
/// For the `VecAdd` shared fixture this produces `Mmap2Stream` (in
/// `SLOT_X0Y2`, whose alphabetical name starts with `SLOT_X0`) as the
/// first-seen leaf — winning `ap_ST_fsm_state*` = `10'd*` over `Add`'s
/// `3'd*`. For a slot with `zleaf` listed before `aleaf` in the raw
/// JSON, sorts them so `aleaf` wins (alphabetical).
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

    // equivalent iteration: for each slot (in top.tasks order),
    // for each leaf (in slot.tasks order), collect RTL parameters,
    // preserving the first `ModuleParameter` seen for each name.
    let mut aggregated: Vec<tapa_graphir::ModuleParameter> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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

/// Build a `GraphIR` Project from `LoweringInputs`.
///
/// Reads `floorplan.json`, `device_config.json`, and `{top}_control_s_axi.v`
/// from disk. The real `ctrl_s_axi` Verilog is required: a placeholder
/// body would leak through the exporter as a `.v` file downstream
/// tools reject.
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
        let module = tapa_rtl::VerilogModule::parse(&body)
            .map_err(|e| LoweringError::MissingLeafRtl(format!("{}: {e}", path.display())))?;
        state
            .attach_module(name, module)
            .map_err(|e| LoweringError::MissingLeafRtl(format!("{name}: {e}")))?;
    }

    // Attach generated upper RTL when present. The post-pass in
    // `build_project_from_state` uses the real top module ports as the
    // boundary and rebuilds equivalent top wires. Without this,
    // GraphIR export keeps the topology-only top definition and misses
    // slot queue-tail / cross-slot FIFO wires referenced by submodules.
    let upper_rtl_task_names: Vec<String> = state
        .program
        .tasks
        .keys()
        .filter(|name| state.is_upper_task(name) && !state.module_map.contains_key(*name))
        .cloned()
        .collect();
    for name in &upper_rtl_task_names {
        let path = rtl_dir.join(format!("{name}.v"));
        if !path.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let module = tapa_rtl::VerilogModule::parse(&body)
            .map_err(|e| LoweringError::MissingLeafRtl(format!("{}: {e}", path.display())))?;
        state
            .attach_module(name, module)
            .map_err(|e| LoweringError::MissingLeafRtl(format!("{name}: {e}")))?;
    }

    // Attach real FSM RTL for every upper task whose `{task}_fsm.v`
    // exists on disk. This lets lowering produce a GraphIR project
    // whose FSM module definitions carry the full Vitis port list
    // (ap_start, ap_done, slot-prefixed handshake ports, …) instead
    // of the 6-port stub the fallback `create_fsm_module`
    // synthesizes.
    //
    // Missing or malformed FSM RTL is surfaced as
    // `LoweringError::MissingFsmRtl` rather than silently falling
    // back to the 6-port stub — otherwise downstream wiring / iface
    // compatibility would silently diverge from with the root cause
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
        let module = tapa_rtl::VerilogModule::parse(&body)
            .map_err(|e| LoweringError::MissingFsmRtl(format!("{}: {e}", fsm_path.display())))?;
        let mut fsm_module = tapa_rtl::mutation::MutableModule::from_parsed(module);
        fsm_module.promote_procedural_output_ports(&body);
        // Drop any stub that was already attached so the real RTL wins.
        state.fsm_modules.remove(task_name);
        state.fsm_modules.insert(task_name.clone(), fsm_module);
    }

    // Read the config inputs using the bundled paths. `?` propagates both
    // missing-file and malformed-JSON errors instead of silently producing
    // `None` values.
    let ctrl_s_axi_path = rtl_dir.join(format!("{}_control_s_axi.v", state.program.top));
    let ctrl_s_axi_body = std::fs::read_to_string(&ctrl_s_axi_path)
        .map_err(|_| LoweringError::MissingCtrlSAxi(ctrl_s_axi_path.display().to_string()))?;

    // Derive slot-to-instances. When the program's topology carries
    // `slot_task_name_to_fp_region`, use the slot-task hierarchy as
    // authoritative (matching pre-baked slot task names like
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
/// task definitions. Matches `_build_program` convention.
fn slot_to_instances_from_topology(program: &Program) -> BTreeMap<String, Vec<String>> {
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
                insts.iter().enumerate().map(move |(idx, inst)| {
                    crate::instantiation_builder::instance_name(child_task_name, idx, inst)
                })
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
    // actual parameter expressions, matching current
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

#[cfg(test)]
#[path = "project_builder_tests.rs"]
mod tests;
