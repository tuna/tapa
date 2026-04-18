//! Static infrastructure module definitions: `ctrl_s_axi`, FIFO template, FSM, reset inverter.

use tapa_graphir::{
    AnyModuleDefinition, Expression, HierarchicalName, ModuleInstantiation, ModuleParameter, Range,
};

use crate::utils::{input_wire, make_connection, output_wire, range_expr, range_msb};

/// Embedded copy of the canonical FIFO Verilog template (shared with Python pipeline).
const FIFO_TEMPLATE: &str = include_str!("../../../tapa/assets/verilog/fifo.v");

/// Embedded reset-inverter Verilog body (matches Python `RESET_INVERTER_TEMPLATE`).
const RESET_INVERTER_TEMPLATE: &str = "
// Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

module reset_inverter (
  input wire clk,
  input wire rst_n,
  output wire rst
);

  assign rst = ~rst_n;

endmodule
";

/// Build the `ctrl_s_axi` module definition from raw Verilog source.
///
/// Creates a `VerilogModuleDefinition` with fixed AXI control parameters and ports,
/// plus dynamic output ports for each top-level task scalar/MMAP-offset arg.
/// Matches Python's `get_ctrl_s_axi_def` which adds a 64-bit output for each
/// top port (scalars as-is, MMAP as `{name}_offset`).
#[must_use]
pub fn get_ctrl_s_axi_def(
    name: &str,
    verilog_source: &str,
    top_ports: &[tapa_topology::task::PortDesign],
) -> AnyModuleDefinition {
    let params = vec![
        ModuleParameter {
            name: "C_S_AXI_ADDR_WIDTH".into(),
            hierarchical_name: HierarchicalName::get_name("C_S_AXI_ADDR_WIDTH"),
            expr: Expression::new_lit("6"),
            range: None,
            extra: std::collections::BTreeMap::default(),
        },
        ModuleParameter {
            name: "C_S_AXI_DATA_WIDTH".into(),
            hierarchical_name: HierarchicalName::get_name("C_S_AXI_DATA_WIDTH"),
            expr: Expression::new_lit("32"),
            range: None,
            extra: std::collections::BTreeMap::default(),
        },
    ];

    let mut ports = vec![
        input_wire("ACLK", None),
        input_wire("ARESET", None),
        input_wire("ACLK_EN", None),
        // AXI write address
        input_wire("AWVALID", None),
        output_wire("AWREADY", None),
        input_wire("AWADDR", Some(range_expr("C_S_AXI_ADDR_WIDTH - 1", "0"))),
        // AXI write data
        input_wire("WVALID", None),
        output_wire("WREADY", None),
        input_wire("WDATA", Some(range_expr("C_S_AXI_DATA_WIDTH - 1", "0"))),
        input_wire("WSTRB", Some(range_expr("C_S_AXI_DATA_WIDTH / 8 - 1", "0"))),
        // AXI read address
        input_wire("ARVALID", None),
        output_wire("ARREADY", None),
        input_wire("ARADDR", Some(range_expr("C_S_AXI_ADDR_WIDTH - 1", "0"))),
        // AXI read data
        output_wire("RVALID", None),
        input_wire("RREADY", None),
        output_wire("RDATA", Some(range_expr("C_S_AXI_DATA_WIDTH - 1", "0"))),
        output_wire("RRESP", Some(range_msb(1))),
        // AXI write response
        output_wire("BVALID", None),
        input_wire("BREADY", None),
        output_wire("BRESP", Some(range_msb(1))),
        // Control signals
        output_wire("ap_start", None),
        input_wire("ap_done", None),
        input_wire("ap_ready", None),
        input_wire("ap_idle", None),
        output_wire("interrupt", None),
    ];

    // Add dynamic output ports for each top-level scalar/MMAP arg.
    // Python: for port_name, port in top.ports.items(): ...
    // Streams are not exposed through ctrl_s_axi.
    let bit64_range = Some(range_msb(63));
    for port in top_ports {
        use tapa_task_graph::port::ArgCategory;
        let port_name = match port.cat {
            ArgCategory::Scalar => port.name.clone(),
            ArgCategory::Mmap | ArgCategory::AsyncMmap
            | ArgCategory::Immap | ArgCategory::Ommap => format!("{}_offset", port.name),
            ArgCategory::Istream | ArgCategory::Ostream
            | ArgCategory::Istreams | ArgCategory::Ostreams => continue,
        };
        ports.push(output_wire(&port_name, bit64_range.clone()));
    }

    AnyModuleDefinition::Verilog {
        base: tapa_graphir::BaseFields {
            name: name.to_owned(),
            hierarchical_name: HierarchicalName::none(),
            parameters: params,
            ports,
            metadata: None,
        },
        verilog: tapa_graphir::VerilogFields {
            verilog: verilog_source.to_owned(),
            submodules_module_names: Vec::new(),
        },
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build the FIFO template module definition.
#[must_use]
pub fn get_fifo_def() -> AnyModuleDefinition {
    let params = vec![
        ModuleParameter {
            name: "DATA_WIDTH".into(),
            hierarchical_name: HierarchicalName::get_name("DATA_WIDTH"),
            expr: Expression::new_lit("32"),
            range: None,
            extra: std::collections::BTreeMap::default(),
        },
        ModuleParameter {
            name: "ADDR_WIDTH".into(),
            hierarchical_name: HierarchicalName::get_name("ADDR_WIDTH"),
            expr: Expression::new_lit("5"),
            range: None,
            extra: std::collections::BTreeMap::default(),
        },
        ModuleParameter {
            name: "DEPTH".into(),
            hierarchical_name: HierarchicalName::get_name("DEPTH"),
            expr: Expression::new_lit("32"),
            range: None,
            extra: std::collections::BTreeMap::default(),
        },
    ];

    let data_range = Range {
        left: Expression(vec![
            tapa_graphir::Token::new_id("DATA_WIDTH"),
            tapa_graphir::Token::new_lit("-"),
            tapa_graphir::Token::new_lit("1"),
        ]),
        right: Expression::new_lit("0"),
    };

    // Python's `get_fifo_def` uses `hierarchical_name = rst` for the
    // `reset` port (the downstream reset wire name). Everything else
    // uses name==hierarchical_name.
    let reset_port = tapa_graphir::ModulePort {
        name: "reset".into(),
        hierarchical_name: HierarchicalName::get_name("rst"),
        port_type: "input wire".into(),
        range: None,
        extra: std::collections::BTreeMap::default(),
    };
    let ports = vec![
        input_wire("clk", None),
        reset_port,
        output_wire("if_full_n", None),
        input_wire("if_write_ce", None),
        input_wire("if_write", None),
        input_wire("if_din", Some(data_range.clone())),
        output_wire("if_empty_n", None),
        input_wire("if_read_ce", None),
        input_wire("if_read", None),
        output_wire("if_dout", Some(data_range)),
    ];

    AnyModuleDefinition::Verilog {
        base: tapa_graphir::BaseFields {
            name: "fifo".into(),
            hierarchical_name: HierarchicalName::none(),
            parameters: params,
            ports,
            metadata: None,
        },
        verilog: tapa_graphir::VerilogFields {
            verilog: FIFO_TEMPLATE.to_owned(),
            submodules_module_names: Vec::new(),
        },
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build the reset inverter module definition.
#[must_use]
pub fn get_reset_inverter_def() -> AnyModuleDefinition {
    AnyModuleDefinition::new_verilog(
        "reset_inverter".into(),
        vec![
            input_wire("clk", None),
            input_wire("rst_n", None),
            output_wire("rst", None),
        ],
        RESET_INVERTER_TEMPLATE.to_owned(),
    )
}

/// Build a reset inverter instantiation.
///
/// Port connections match Python's `get_reset_inverter_inst` in
/// `tapa/graphir_conversion/module_definitions.py`: `clk` → `ap_clk`,
/// `rst_n` → `ap_rst_n`, and the inverter output port `rst` → the
/// top-level `rst` wire (not `ap_rst`).
#[must_use]
pub fn get_reset_inverter_inst(region: Option<&str>) -> ModuleInstantiation {
    ModuleInstantiation {
        name: "reset_inverter_0".into(),
        hierarchical_name: HierarchicalName::get_name("reset_inverter_0"),
        module: "reset_inverter".into(),
        connections: vec![
            make_connection("clk", Expression::new_id("ap_clk")),
            make_connection("rst_n", Expression::new_id("ap_rst_n")),
            make_connection("rst", Expression::new_id("rst")),
        ],
        parameters: Vec::new(),
        floorplan_region: region.map(str::to_owned),
        area: None,
        pragmas: Vec::new(),
        extra: std::collections::BTreeMap::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_s_axi_def_has_ports() {
        let def = get_ctrl_s_axi_def("ctrl_s_axi", "// dummy", &[]);
        assert_eq!(def.name(), "ctrl_s_axi");
        assert!(def.ports().len() >= 20, "should have many AXI ports");
    }

    #[test]
    fn ctrl_s_axi_def_includes_scalar_ports() {
        let top_ports: Vec<tapa_topology::task::PortDesign> = serde_json::from_str(r#"[
            {"cat": "scalar", "name": "n", "type": "int", "width": 32}
        ]"#).unwrap();
        let def = get_ctrl_s_axi_def("ctrl_s_axi", "// dummy", &top_ports);
        let port_names: Vec<_> = def.ports().iter().map(|p| p.name.clone()).collect();
        assert!(port_names.contains(&"n".to_owned()), "should include scalar port n, got: {port_names:?}");
    }

    #[test]
    fn fifo_def_has_parameters() {
        let def = get_fifo_def();
        assert_eq!(def.name(), "fifo");
        if let AnyModuleDefinition::Verilog { base, verilog, .. } = &def {
            assert_eq!(base.parameters.len(), 3);
            assert!(
                verilog.verilog.contains("module fifo"),
                "fifo def must carry real template body, got {:?}",
                &verilog.verilog[..verilog.verilog.len().min(80)],
            );
        } else {
            panic!("should be Verilog");
        }
    }

    #[test]
    fn reset_inverter_def_carries_template() {
        let def = get_reset_inverter_def();
        if let AnyModuleDefinition::Verilog { verilog, .. } = &def {
            assert!(verilog.verilog.contains("module reset_inverter"));
            assert!(verilog.verilog.contains("assign rst = ~rst_n"));
        } else {
            panic!("should be Verilog");
        }
    }

    #[test]
    fn reset_inverter_def_has_ports() {
        let def = get_reset_inverter_def();
        assert_eq!(def.name(), "reset_inverter");
        assert_eq!(def.ports().len(), 3);
    }

    #[test]
    fn reset_inverter_inst_connections() {
        let inst = get_reset_inverter_inst(Some("SLOT_X0Y0"));
        assert_eq!(inst.name, "reset_inverter_0");
        assert_eq!(inst.connections.len(), 3);
        assert_eq!(inst.floorplan_region.as_deref(), Some("SLOT_X0Y0"));
    }
}
