//! Python-equivalent wire synthesis for upper (slot + top) grouped modules.
//!
//! Mirrors `tapa/graphir_conversion/pipeline/wire_builder.py::get_upper_task_ir_wires`
//! and `tapa/graphir_conversion/gen_rs_graphir.py::get_top_extra_wires`.

use std::collections::BTreeMap;

use tapa_graphir::{
    AnyModuleDefinition, HierarchicalName, ModuleNet, ModulePort, Range,
};
use tapa_task_graph::port::ArgCategory;
use tapa_topology::task::TaskDesign;

/// Build the three-category wire list Python's `get_upper_task_ir_wires`
/// emits for an upper task (slot or top):
///
/// 1. Local FIFO wires (non-external FIFOs) — six wires per FIFO:
///    `{sanitized_name}{suffix}` for `_dout`/`_empty_n`/`_read`/`_din`/`_full_n`/`_write`.
///    Data suffixes (`_dout`, `_din`) get the FIFO data range; control
///    suffixes get `None`.
/// 2. Arg-table queue-tail wires — one wire per non-stream arg per child
///    instance, named `{inst}___{arg}_offset__q0` for mmap and
///    `{inst}___{arg}__q0` for scalar. Ranges come from the upper
///    task's own port or — as fallback — the `{arg}_offset` entry in the
///    upper port table; `ctrl_s_axi` ports are included in the range-lookup
///    set for the top task.
/// 3. Per-instance control wires — four wires per child instance:
///    `{inst}__ap_start`, `{inst}__ap_done`, `{inst}__ap_ready`,
///    `{inst}__ap_idle`.
///
/// The returned list preserves Python's emission order (FIFOs → arg-table
/// wires → per-instance controls).
#[must_use]
pub fn build_upper_task_ir_wires(
    upper_task: &TaskDesign,
    upper_task_ports: &[ModulePort],
    ctrl_s_axi_ir_ports: &[ModulePort],
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
) -> Vec<ModuleNet> {
    let mut wires: Vec<ModuleNet> = Vec::new();

    // 1. Local FIFO wires. Intra-upper-task FIFOs have both `produced_by`
    //    and `consumed_by` set. `is_fifo_external_codegen` (Python) treats
    //    only those as internal.
    for (fifo_name, fifo) in &upper_task.fifos {
        if fifo.produced_by.is_none() || fifo.consumed_by.is_none() {
            continue;
        }
        let sanitized = tapa_rtl::module::sanitize_array_name(fifo_name);
        let data_range = infer_fifo_data_range(fifo_name, fifo, upper_task, leaf_modules, false);
        for &suffix in &["_dout", "_empty_n", "_read", "_din", "_full_n", "_write"] {
            let wire_name = format!("{sanitized}{suffix}");
            let range = if matches!(suffix, "_dout" | "_din") {
                data_range.clone()
            } else {
                None
            };
            wires.push(make_net(&wire_name, range));
        }
    }

    // Port-range mapping: upper_task's own ports + ctrl_s_axi ports.
    let mut port_range_mapping: BTreeMap<String, Option<Range>> = BTreeMap::new();
    for p in upper_task_ports.iter().chain(ctrl_s_axi_ir_ports.iter()) {
        port_range_mapping.insert(p.name.clone(), p.range.clone());
    }

    // 2. Arg-table queue-tail wires.
    for (child_task_name, insts) in &upper_task.tasks {
        for (idx, inst) in insts.iter().enumerate() {
            let inst_name = format!("{child_task_name}_{idx}");
            for arg in inst.args.values() {
                let wire_name = match arg.cat {
                    ArgCategory::Scalar => format!("{inst_name}___{}__q0", arg.arg),
                    ArgCategory::Mmap
                    | ArgCategory::AsyncMmap
                    | ArgCategory::Immap
                    | ArgCategory::Ommap => format!("{inst_name}___{}_offset__q0", arg.arg),
                    ArgCategory::Istream
                    | ArgCategory::Ostream
                    | ArgCategory::Istreams
                    | ArgCategory::Ostreams => continue,
                };
                // Python: port_range_key = arg if arg in mapping else f"{arg}_offset"
                let range = port_range_mapping
                    .get(&arg.arg)
                    .cloned()
                    .unwrap_or_else(|| {
                        port_range_mapping
                            .get(&format!("{}_offset", arg.arg))
                            .cloned()
                            .unwrap_or(None)
                    });
                wires.push(make_net(&wire_name, range));
            }
        }
    }

    // 3. Per-instance control wires.
    for (child_task_name, insts) in &upper_task.tasks {
        for idx in 0..insts.len() {
            let inst_name = format!("{child_task_name}_{idx}");
            for &sig in &["ap_start", "ap_done", "ap_ready", "ap_idle"] {
                wires.push(make_net(&format!("{inst_name}__{sig}"), None));
            }
        }
    }

    wires
}

/// Extra top-only wires contributed by the `ctrl_s_axi` module.
///
/// Every port of `ctrl_s_axi` that is not in the port-mapping set
/// Python's `gen_rs_graphir._CTRL_S_AXI_PORT_MAPPING` covers becomes an
/// internal top-module wire. The mapping keys are:
///
///   * the 17 `s_axi_control_*` AXI-Lite port names (which get routed to
///     identically-named top ports),
///   * `ACLK`, `ARESET`, `ACLK_EN` (routed to `ap_clk`, `rst`, `1'b1`).
///
/// Everything else — `ap_start`, `ap_done`, `ap_ready`, `ap_idle`,
/// `interrupt`, and the dynamic scalar/MMAP-offset output ports — stays
/// as an internal wire between `ctrl_s_axi` and the rest of the top
/// grouped module.
#[must_use]
pub fn build_top_extra_wires(ctrl_s_axi_ports: &[ModulePort]) -> Vec<ModuleNet> {
    let axi_ports: &[&str] = &[
        "AWVALID", "AWREADY", "AWADDR", "WVALID", "WREADY", "WDATA", "WSTRB", "ARVALID",
        "ARREADY", "ARADDR", "RVALID", "RREADY", "RDATA", "RRESP", "BVALID", "BREADY", "BRESP",
    ];
    let mapped: std::collections::BTreeSet<&str> = axi_ports
        .iter()
        .copied()
        .chain(["ACLK", "ARESET", "ACLK_EN"])
        .collect();
    ctrl_s_axi_ports
        .iter()
        .filter(|p| !mapped.contains(p.name.as_str()))
        .map(|p| make_net(&p.name, p.range.clone()))
        .collect()
}

/// Try to infer the data range of a FIFO by looking up the producer's
/// `_din` port on the submodule IR. Mirrors Python's
/// `tapa.graphir_conversion.pipeline.fifo_builder.infer_fifo_data_range`.
///
/// `is_top` mirrors Python's `infer_port_name_from_tapa_module=not is_top`:
///   * `is_top=false`: use the producer's child RTL `get_port_of` lookup
///     (applies `_FIFO_INFIXES` normalization) for slot-local FIFOs.
///   * `is_top=true`: use the plain `{fifo_name}_din` port name, since the
///     producer is a slot grouped module whose port name is already the
///     parent-visible fifo name.
#[must_use]
pub fn infer_fifo_data_range(
    fifo_name: &str,
    fifo: &tapa_topology::task::FifoDesign,
    upper_task: &TaskDesign,
    submodule_ir_defs: &BTreeMap<String, AnyModuleDefinition>,
    is_top: bool,
) -> Option<Range> {
    let endpoint = fifo.produced_by.as_ref()?;
    let producer_task_name = &endpoint.0;
    let producer_def = submodule_ir_defs.get(producer_task_name)?;

    if is_top {
        // Python: producer_data_port = get_stream_port_name(producer_fifo, "_din")
        let sanitized = tapa_rtl::module::sanitize_array_name(fifo_name);
        let data_port_name = format!("{sanitized}_din");
        return producer_def
            .ports()
            .iter()
            .find(|p| p.name == data_port_name)
            .and_then(|p| p.range.clone());
    }

    // Slot-local FIFO: find the child arg name, then try each FIFO_INFIXES
    // suffix pattern (matches Python's `rtl_module.get_port_of(arg, "_din")`).
    let producer_port_name = upper_task
        .tasks
        .get(producer_task_name)
        .and_then(|insts| {
            insts.iter().find_map(|inst| {
                inst.args
                    .iter()
                    .find(|(_, arg)| arg.arg == *fifo_name)
                    .map(|(child_port, _)| child_port.clone())
            })
        })?;
    find_port_with_infixes(producer_def, &producer_port_name, "_din")
        .and_then(|p| p.range.clone())
}

fn find_port_with_infixes<'a>(
    def: &'a AnyModuleDefinition,
    base: &str,
    suffix: &str,
) -> Option<&'a tapa_graphir::ModulePort> {
    let sanitized = tapa_rtl::module::sanitize_array_name(base);
    for infix in tapa_rtl::module::FIFO_INFIXES {
        let candidate = format!("{sanitized}{infix}{suffix}");
        if let Some(port) = def.ports().iter().find(|p| p.name == candidate) {
            return Some(port);
        }
    }
    None
}

/// Infer a top-level cross-slot FIFO's data range via the leaf producer.
///
/// Drills into the producer slot's child leaf RTL that actually
/// produces the FIFO. The slot's topology-synthesized port ranges
/// aren't Python-equivalent at the time `build_top_module` runs (slot
/// ports get rewritten in a later post-pass), so we bypass the slot
/// def and look up the leaf producer directly.
#[must_use]
pub fn infer_top_fifo_data_range_via_leaf(
    fifo_name: &str,
    fifo: &tapa_topology::task::FifoDesign,
    program: &tapa_topology::program::Program,
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
) -> Option<Range> {
    let endpoint = fifo.produced_by.as_ref()?;
    let producer_slot_name = &endpoint.0;
    let producer_slot_task = program.tasks.get(producer_slot_name)?;
    for (leaf_task_name, insts) in &producer_slot_task.tasks {
        for inst in insts {
            for (child_port, arg) in &inst.args {
                if arg.arg == *fifo_name {
                    let leaf_def = leaf_modules.get(leaf_task_name)?;
                    if let Some(range) = find_port_with_infixes(leaf_def, child_port, "_din")
                        .and_then(|p| p.range.clone())
                    {
                        return Some(range);
                    }
                }
            }
        }
    }
    None
}

fn make_net(name: &str, range: Option<Range>) -> ModuleNet {
    ModuleNet {
        name: name.to_owned(),
        hierarchical_name: HierarchicalName::get_name(name),
        range,
        extra: BTreeMap::default(),
    }
}
