//! Top-task `control_s_axi` instantiation.

use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_START,
    S_AXI_LITE_CTRL_PORTS, S_AXI_NAME,
};
use tapa_rtl::builder::{Expr, ParamArg, PortArg};
use tapa_rtl::mutation::{wide_wire, wire};

use crate::rtl_state::TopologyWithRtl;

pub fn instantiate_top_control_s_axi(state: &mut TopologyWithRtl, task_name: &str) {
    if task_name != state.design.top || !state.top_instantiates_control_s_axi() {
        return;
    }
    let Some(task) = state.design.tasks.get(task_name) else {
        return;
    };
    let Some(mm) = state.module_map.get_mut(task_name) else {
        return;
    };

    let mut ports = vec![
        PortArg::new("ACLK", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("ARESET", Expr::ident(HANDSHAKE_RST)),
        PortArg::new("ACLK_EN", Expr::lit("1'b1")),
    ];
    for &axi_port in S_AXI_LITE_CTRL_PORTS {
        ports.push(PortArg::new(
            axi_port,
            Expr::ident(format!("{S_AXI_NAME}_{axi_port}")),
        ));
    }
    for &sig in &[
        HANDSHAKE_START,
        HANDSHAKE_DONE,
        HANDSHAKE_IDLE,
        HANDSHAKE_READY,
        "interrupt",
    ] {
        ports.push(PortArg::new(sig, Expr::ident(sig)));
    }

    for port in &task.ports {
        let sanitized = tapa_rtl::module::sanitize_array_name(&port.name);
        let ctrl_port_names = if port.cat.is_scalar() {
            let width = port.width.max(1);
            if width == 1 {
                let _ = mm.add_signal(wire(&sanitized));
            } else {
                let _ = mm.add_signal(wide_wire(&sanitized, &(width - 1).to_string(), "0"));
            }
            vec![sanitized]
        } else if port.cat.is_mmap_like() {
            if let Some(chan_count) = port.chan_count {
                (0..chan_count)
                    .map(|idx| {
                        let name = format!("{sanitized}_{idx}_offset");
                        let _ = mm.add_signal(wide_wire(&name, "63", "0"));
                        name
                    })
                    .collect()
            } else {
                let name = format!("{sanitized}_offset");
                let _ = mm.add_signal(wide_wire(&name, "63", "0"));
                vec![name]
            }
        } else {
            // Streams contribute no s_axilite control ports.
            Vec::new()
        };
        for ctrl_port_name in ctrl_port_names {
            ports.push(PortArg::new(
                ctrl_port_name.clone(),
                Expr::ident(ctrl_port_name),
            ));
        }
    }

    let inst = tapa_rtl::builder::ModuleInstance::new(
        format!("{task_name}_control_s_axi"),
        "control_s_axi_U",
    )
    .with_params(vec![
        ParamArg::new(
            "C_S_AXI_ADDR_WIDTH",
            Expr::ident("C_S_AXI_CONTROL_ADDR_WIDTH"),
        ),
        ParamArg::new(
            "C_S_AXI_DATA_WIDTH",
            Expr::ident("C_S_AXI_CONTROL_DATA_WIDTH"),
        ),
    ])
    .with_ports(ports);
    mm.add_instance(inst);
}
