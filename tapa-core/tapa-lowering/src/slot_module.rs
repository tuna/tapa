//! Slot grouped-module assembly.

use std::collections::BTreeMap;

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_graphir::{AnyModuleDefinition, ModulePort};
use tapa_topology::program::Program;

use crate::instantiation_builder::{
    build_fifo_instance, build_port_connections, build_task_instance, ArgTable,
};
use crate::utils::{
    attach_grouped_assigns, build_arg_pipeline_assigns, find_arg_name_in_task,
    instance_matches_name, parse_instance_name, port_range, resolve_instance_in_task,
};
use crate::utils::{input_wire, make_wire, range_msb};
use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
    HANDSHAKE_START,
};

/// Build a slot module definition containing task instances and FIFOs.
#[allow(clippy::too_many_lines, reason = "sequential slot assembly logic")]
pub fn build_slot_module(
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
    // fall back to the slot name.
    let fp_region = program
        .slot_task_name_to_fp_region
        .as_ref()
        .and_then(|m| m.get(slot_name).cloned())
        .unwrap_or_else(|| slot_name.to_owned());
    // Slot-local arg table for this slot's children:
    // `get_task_arg_table(slot)` used in instantiation_builder
    // for slot grouped modules — Rust previously built arg tables from
    // the top task, which means child leaf instances inside a slot had
    // no arg entries and `build_port_connections` fell back to raw arg
    // names instead of `{inst}___{arg}[_offset]__q0` queue-tail
    // signals. When a slot_name does not correspond to a registered
    // program task (small test fixtures), fall back to the top task for
    // the arg-table context so the builder still produces a compatible
    // shape.
    let top = &program.tasks[&program.top];
    let slot_task_ref = program.tasks.get(slot_name).unwrap_or(top);
    let slot_arg_table = crate::instantiation_builder::build_arg_table(slot_task_ref);
    let mut ports = vec![
        input_wire(HANDSHAKE_CLK, None),
        input_wire(HANDSHAKE_RST_N, None),
        input_wire(HANDSHAKE_START, None),
        crate::utils::output_wire(HANDSHAKE_DONE, None),
        crate::utils::output_wire(HANDSHAKE_IDLE, None),
        crate::utils::output_wire(HANDSHAKE_READY, None),
    ];
    let mut submodules = Vec::new();
    let mut wires = Vec::new();
    let mut direct_assigns = Vec::new();

    // slot grouped modules do not contain a reset_inverter
    // instance; the reset_inverter is a top-level instance only. Keep the
    // `ap_rst` wire declaration for local signals that reference it, but
    // skip the per-slot instantiation.
    wires.push(make_wire(HANDSHAKE_RST, None));

    // Add pipeline wires from arg table for instances in this slot
    for inst_name in inst_names {
        let canonical_inst_name = resolve_instance_in_task(slot_task_ref, inst_name)
            .map_or_else(|| inst_name.clone(), |(_, canonical)| canonical);
        if let Some(inst_signals) = slot_arg_table
            .get(&canonical_inst_name)
            .or_else(|| arg_table.get(&canonical_inst_name))
        {
            for signal in inst_signals.values() {
                if !wires.iter().any(|w| w.name == *signal) {
                    wires.push(make_wire(signal, None));
                }
            }
        }
    }

    // Add task instances belonging to this slot
    for inst_name in inst_names {
        // Resolve explicit instance labels through the parent task's
        // child map before falling back to the legacy "taskname_idx"
        // parse. Labels like `Module2Func_1` may still instantiate
        // task/module `Module1Func`.
        let (task_name, canonical_inst_name) = resolve_instance_in_task(slot_task_ref, inst_name)
            .unwrap_or_else(|| {
                let (task_name, _idx) = parse_instance_name(inst_name);
                (task_name, inst_name.clone())
            });
        let inst_name = &canonical_inst_name;
        if let Some(task) = program.tasks.get(&task_name) {
            // Add control wires for this instance
            for sig in &[
                HANDSHAKE_START,
                HANDSHAKE_DONE,
                HANDSHAKE_IDLE,
                HANDSHAKE_READY,
            ] {
                wires.push(make_wire(&format!("{inst_name}__{sig}"), None));
            }

            // Collect child RTL port names for MMAP filtering
            let child_rtl_ports: Option<std::collections::HashSet<String>> = leaf_modules
                .get(&task_name)
                .map(|def| def.ports().iter().map(|p| p.name.clone()).collect());

            // Find the instance's args in the SLOT task, using the
            // slot-local arg table for pipeline routing:
            // `get_upper_module_ir_subinsts(slot, ...)` which walks
            // `slot.instances`, not the top's.
            let inst_arg_table = slot_arg_table.get(inst_name);
            let mut arg_connections = Vec::new();
            if let Some(instances) = slot_task_ref.tasks.get(&task_name) {
                for (idx, inst) in instances.iter().enumerate() {
                    if instance_matches_name(&task_name, idx, inst, inst_name) {
                        for (port_name, arg) in &inst.args {
                            // Connect child port through the slot-local arg
                            // table for all categories. Scalars route through
                            // queue-tail wires ({inst}___{arg}__q0); mmap
                            // offsets through ({inst}___{arg}_offset__q0),
                            // matching `_connect_scalar` +
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
                            if matches!(arg.cat, tapa_task_graph::port::ArgCategory::Scalar) {
                                let width = task
                                    .ports
                                    .iter()
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
                // hierarchy), then fall back to the top task's map. current
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
                    if port.cat == tapa_task_graph::port::ArgCategory::AsyncMmap {
                        // Async mmap leaves expose FIFO-style channels to the
                        // generated async_mmap bridge, but the slot boundary
                        // must expose the bridge's parent-facing M-AXI ports.
                        for expanded in
                            crate::utils::expand_port_to_signals(&arg_name, port.cat, port.width)
                        {
                            if !ports.iter().any(|p: &ModulePort| p.name == expanded.name) {
                                ports.push(expanded);
                            }
                        }
                        continue;
                    }
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
                                let is_child_output = leaf_modules
                                    .get(&task_name)
                                    .and_then(|def| {
                                        def.ports().iter().find(|p| p.name == child_port)
                                    })
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
                for expanded in
                    crate::utils::expand_port_to_signals(&arg_name, port.cat, port.width)
                {
                    if !ports.iter().any(|p: &ModulePort| p.name == expanded.name) {
                        ports.push(expanded);
                    }
                }
            }

            let module_name = leaf_modules
                .get(&task_name)
                .map_or(task_name.as_str(), AnyModuleDefinition::name);
            submodules.push(build_task_instance(
                inst_name,
                module_name,
                arg_connections,
                Some(&fp_region),
            ));
        }
    }

    // Add the slot FSM instance unconditionally. current
    // `get_upper_module_ir_subinsts` appends
    // `_make_fsm_inst(upper_task.rtl_fsm_module, floorplan_region)` at
    // this point — a self-connected instance named `{slot}_fsm_0` that
    // references every FSM port. Any FSM port that isn't already a slot
    // port or wire gets added as a wire so the exporter's DRC can find
    // every identifier.
    let slot_fsm_name = format!("{slot_name}_fsm");
    if let Some(fsm_def) = fsm_modules.get(&slot_fsm_name) {
        submodules.push(crate::instantiation_builder::build_self_connected_fsm_inst(
            fsm_def,
            &slot_fsm_name,
            Some(fp_region.clone()),
            &ports,
            &mut wires,
        ));
        direct_assigns.extend(build_arg_pipeline_assigns(
            slot_task_ref,
            fsm_def,
            &slot_arg_table,
        ));
    }

    // Add FIFO instances for FIFOs whose producer and consumer both live
    // inside this slot. `get_upper_module_ir_subinsts` iterates
    // `upper_task.fifos` (the slot's own FIFO map, not the top task's)
    // and keeps only the internal ones via `is_fifo_external_codegen`.
    // Intra-slot FIFOs are those with both `produced_by` and `consumed_by`
    // set.
    if let Some(slot_task) = program.tasks.get(slot_name) {
        for (fifo_name, fifo) in &slot_task.fifos {
            if fifo.produced_by.is_none() || fifo.consumed_by.is_none() {
                continue;
            }
            let data_range = crate::upper_wires::infer_fifo_data_range(
                fifo_name,
                fifo,
                slot_task,
                leaf_modules,
                false,
            );
            crate::utils::declare_fifo_wires(&mut wires, &ports, fifo_name, data_range.as_ref());
            let depth = fifo.depth.unwrap_or(32);
            // Producer is a child leaf; look up the _din range on the
            // child RTL via get_port_of normalization.
            submodules.push(build_fifo_instance(
                fifo_name,
                data_range.as_ref(),
                depth,
                Some(&fp_region),
                false,
            ));
        }
    }

    let mut module =
        AnyModuleDefinition::new_grouped(slot_name.to_owned(), ports, submodules, wires);
    attach_grouped_assigns(&mut module, direct_assigns);
    module
}
