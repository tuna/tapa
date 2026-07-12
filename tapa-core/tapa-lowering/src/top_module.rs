//! Top grouped-module assembly (incl. `ctrl_s_axi` routing).

use std::collections::BTreeMap;

use tapa_graphir::{AnyModuleDefinition, Expression, HierarchicalName};
use tapa_topology::program::Program;

use crate::instantiation_builder::build_fifo_instance;
use crate::module_defs::get_reset_inverter_inst;
use crate::utils::{attach_grouped_assigns, build_arg_pipeline_assigns};
use crate::utils::{input_wire, make_wire, range_msb};
use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST_N,
    HANDSHAKE_START, S_AXI_LITE_CTRL_PORTS, S_AXI_NAME,
};

/// Returns true if the named AXI-Lite control port is an input on the
/// slave (top-level) side: master→slave address/data/valid channels and
/// the response-channel READY ports.
fn is_s_axi_slave_input(axi_port: &str) -> bool {
    tapa_protocol::S_AXI_LITE_PORT_DIRS
        .iter()
        .any(|&(name, dir)| name == axi_port && dir == tapa_protocol::PortDir::Input)
}

/// Port mapping from `ctrl_s_axi` internal name → top-level expression.
/// `_CTRL_S_AXI_PORT_MAPPING` in `gen_rs_graphir`.
fn ctrl_s_axi_port_expr(port_name: &str) -> Expression {
    match port_name {
        "ACLK" => Expression::new_id(HANDSHAKE_CLK),
        // routes ctrl_s_axi.ARESET through `rst` (output of
        // reset_inverter), same as reset_inverter_0.rst → `rst`.
        "ARESET" => Expression::new_id("rst"),
        "ACLK_EN" => Expression::new_lit("1'b1"),
        _ => {
            // AXI-Lite ports map to s_axi_control_{name} at top level
            if S_AXI_LITE_CTRL_PORTS.contains(&port_name) {
                Expression::new_id(&format!("{S_AXI_NAME}_{port_name}"))
            } else {
                // Control/scalar ports (ap_start, ap_done, etc.) connect to internal wires
                Expression::new_id(port_name)
            }
        }
    }
}

/// Build the top-level module definition with slot instances, FSM, and `ctrl_s_axi`.
#[allow(clippy::too_many_lines, reason = "sequential top-module assembly")]
#[allow(
    clippy::too_many_arguments,
    reason = "top-module assembly orchestrator"
)]
pub fn build_top_module(
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
    // reset_inverter). get_top_module_definition uses the first
    // value of slot_task_name_to_fp_region as default_region; we do the same.
    let default_region = program
        .slot_task_name_to_fp_region
        .as_ref()
        .and_then(|m| m.values().next().cloned());
    let mut ports = vec![
        input_wire(HANDSHAKE_CLK, None),
        input_wire(HANDSHAKE_RST_N, None),
    ];

    // Add s_axi_control_* ports (AXI-Lite slave interface) if ctrl_s_axi is
    // present. Directions match the ctrl_s_axi module's own port list:
    // master→slave ports are inputs (AW/W/AR VALID+ADDR+DATA+STRB, BREADY,
    // RREADY); slave→master ports are outputs (AW/W/AR READY, R VALID+DATA+
    // RESP, B VALID+RESP).
    if has_ctrl_s_axi {
        for &axi_port in S_AXI_LITE_CTRL_PORTS {
            let port_name = format!("{S_AXI_NAME}_{axi_port}");
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
    let mut wires = vec![
        make_wire("rst", None),
        make_wire(HANDSHAKE_START, None),
        make_wire(HANDSHAKE_DONE, None),
        make_wire(HANDSHAKE_IDLE, None),
        make_wire(HANDSHAKE_READY, None),
        make_wire("interrupt", None),
    ];
    let top_arg_table = crate::instantiation_builder::build_arg_table(top);
    let mut direct_assigns = Vec::new();

    // FSM instance — self-connect every port in the FSM module definition
    // (each port expression is just
    // an identifier for the same name). For any FSM port that isn't
    // already declared as a top-level port or wire (e.g. slot-prefixed
    // handshake signals like `SLOT_X0Y2_SLOT_X0Y2_0__ap_start`), emit a
    // matching wire so the exporter's DRC can find every identifier.
    if let Some(fsm_def) = fsm_modules.get(fsm_name) {
        direct_assigns.extend(build_arg_pipeline_assigns(top, fsm_def, &top_arg_table));
        submodules.push(crate::instantiation_builder::build_self_connected_fsm_inst(
            fsm_def,
            fsm_name,
            default_region.clone(),
            &ports,
            &mut wires,
        ));
    } else {
        let top_fsm_connections: Vec<tapa_graphir::ModuleConnection> = [
            HANDSHAKE_CLK,
            HANDSHAKE_RST_N,
            HANDSHAKE_START,
            HANDSHAKE_DONE,
            HANDSHAKE_IDLE,
            HANDSHAKE_READY,
        ]
        .iter()
        .map(|&name| crate::utils::make_connection(name, Expression::new_id(name)))
        .collect();
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
    }

    // ctrl_s_axi instance — maps AXI-Lite ports through s_axi_control_* top ports
    if has_ctrl_s_axi {
        let mut ctrl_connections = Vec::new();
        // Fixed port mappings (clock, reset, enable) + AXI-Lite channel ports.
        // `ctrl_s_axi_port_expr` routes ACLK/ARESET/ACLK_EN and the
        // S_AXI_LITE_CTRL_PORTS set; each gets mapped to its top-level wire.
        for &axi_port in ["ACLK", "ARESET", "ACLK_EN"]
            .iter()
            .chain(S_AXI_LITE_CTRL_PORTS)
        {
            ctrl_connections.push(crate::utils::make_connection(
                axi_port,
                ctrl_s_axi_port_expr(axi_port),
            ));
        }
        // Control signal wires (internal, between FSM and ctrl_s_axi)
        for &sig in &[
            HANDSHAKE_START,
            HANDSHAKE_DONE,
            HANDSHAKE_IDLE,
            HANDSHAKE_READY,
            "interrupt",
        ] {
            ctrl_connections.push(crate::utils::make_connection(sig, Expression::new_id(sig)));
        }
        // Dynamic scalar/MMAP-offset ports — connect to same-name top-level wires
        // current: _CTRL_S_AXI_PORT_MAPPING defaults unknown ports to Token.new_id(port.name)
        for port in &top.ports {
            use tapa_task_graph::port::ArgCategory;
            let ctrl_port_name = match port.cat {
                ArgCategory::Scalar => port.name.clone(),
                ArgCategory::Mmap
                | ArgCategory::AsyncMmap
                | ArgCategory::Immap
                | ArgCategory::Ommap => format!("{}_offset", port.name),
                ArgCategory::Istream
                | ArgCategory::Ostream
                | ArgCategory::Istreams
                | ArgCategory::Ostreams => continue,
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

        // get_top_ctrl_s_axi_inst (gen_rs_graphir) passes two
        // parameter assignments: the ctrl_s_axi module exposes
        // C_S_AXI_ADDR_WIDTH / C_S_AXI_DATA_WIDTH, which the top
        // instantiation ties to the top task's
        // C_S_AXI_CONTROL_ADDR_WIDTH / C_S_AXI_CONTROL_DATA_WIDTH.
        // copies `Expression(top_param_by_name[value].expr.root)`,
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
            floorplan_region: default_region.clone(),
            area: None,
            pragmas: Vec::new(),
            extra: BTreeMap::default(),
        });
    }

    // Slot instances — equivalent `get_top_level_slot_inst`:
    // build connections by walking each slot's args in the TOP task and
    // running them through the same `_connect_scalar` / `_connect_istream`
    // / `_connect_ostream` / `_connect_mmap` flow as child instances.
    // Scalars and mmap offsets route through the TOP task's arg-table
    // queue-tail wires (`{slot_inst}___{arg}[_offset]__q0`), streams
    // through the slot's own boundary port names, and mmap AXI channels
    // through the parent-visible `m_axi_{arg}_*` wire names.
    for slot_name in slot_to_instances.keys() {
        let slot_def = slot_defs.iter().find(|d| d.name() == slot_name);
        let slot_port_names: Option<std::collections::HashSet<String>> =
            slot_def.map(|d| d.ports().iter().map(|p| p.name.clone()).collect());
        let slot_inst_name = top
            .tasks
            .get(slot_name)
            .and_then(|v| v.first())
            .map_or_else(
                || format!("{slot_name}_0"),
                |inst| crate::instantiation_builder::instance_name(slot_name, 0, inst),
            );

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
        // registered (matches `get_top_level_slot_inst`), or
        // the top-level wires otherwise (for trivial fixtures that
        // synthesize "slot" names from floorplan regions rather than
        // from task names).
        slot_connections.push(crate::utils::make_connection(
            HANDSHAKE_CLK,
            Expression::new_id(HANDSHAKE_CLK),
        ));
        slot_connections.push(crate::utils::make_connection(
            HANDSHAKE_RST_N,
            Expression::new_id(HANDSHAKE_RST_N),
        ));
        for sig in &[
            HANDSHAKE_START,
            HANDSHAKE_DONE,
            HANDSHAKE_READY,
            HANDSHAKE_IDLE,
        ] {
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
    // different slots become top-level submodules. current
    // `get_top_ir_subinsts` adds one `fifo` instance per such FIFO,
    // assigned to the consumer's slot region. Matching here closes
    // the submodule-count compatibility gap on the shared fixture.
    let region_map = program.slot_task_name_to_fp_region.as_ref();
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
        // Cross-slot FIFO: drill into the producer slot's child leaf
        // RTL to get the `_din` port range (looks up the
        // slot-def port, but at this point the slot defs have not
        // yet been rewritten with equivalent ports).
        let data_range = crate::upper_wires::infer_top_fifo_data_range_via_leaf(
            fifo_name,
            fifo,
            program,
            leaf_modules,
        );
        crate::utils::declare_fifo_wires(&mut wires, &ports, fifo_name, data_range.as_ref());
        let depth = fifo.depth.unwrap_or(32);
        // Region: prefer the consumer's explicit slot region; if the
        // floorplanned topology carries slot endpoints but no region map,
        // use the consumer slot name as a stable fallback.
        let fifo_region = consumer_slot
            .and_then(|c| {
                region_map
                    .and_then(|m| m.get(c).cloned())
                    .or_else(|| Some(c.clone()))
            })
            .or_else(|| default_region.clone());
        let fifo_inst = build_fifo_instance(
            fifo_name,
            data_range.as_ref(),
            depth,
            fifo_region.as_deref(),
            true,
        );
        submodules.push(fifo_inst);
    }

    let mut module =
        AnyModuleDefinition::new_grouped(program.top.clone(), ports, submodules, wires);
    attach_grouped_assigns(&mut module, direct_assigns);
    module
}
