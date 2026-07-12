//! Port-only RTL shells for tasks implemented through custom RTL.

use tapa_protocol::{
    PortDir, HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST_N,
    HANDSHAKE_START, M_AXI_PORTS, M_AXI_PORT_WIDTHS, M_AXI_PREFIX,
};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{simple_port, wide_port};
use tapa_rtl::port::{Direction, Port};
use tapa_task_graph::port::ArgCategory;
use tapa_topology::task::TaskDesign;

const M_AXI_CHANNEL_ORDER: &[&str] = &["AR", "AW", "B", "R", "W"];

fn port_with_width(name: impl Into<String>, direction: Direction, width: u32) -> Port {
    if width > 1 {
        wide_port(name, direction, &(width - 1).to_string(), "0")
    } else {
        simple_port(name, direction)
    }
}

fn add_m_axi_ports(ports: &mut Vec<Port>, name: &str, data_width: u32) {
    let prefix = format!("{M_AXI_PREFIX}{}", sanitize_array_name(name));
    for &channel in M_AXI_CHANNEL_ORDER {
        let Some(&subports) = M_AXI_PORTS.get(channel) else {
            continue;
        };
        for &(subport, direction) in subports {
            let direction = match direction {
                PortDir::Input => Direction::Input,
                PortDir::Output => Direction::Output,
            };
            let default_width = M_AXI_PORT_WIDTHS.get(subport).copied().unwrap_or(1);
            let width = match subport {
                "ADDR" => 64,
                "DATA" => data_width,
                "ID" => 1,
                "STRB" => data_width.div_ceil(8),
                _ if default_width == 0 => 1,
                _ => default_width,
            };
            ports.push(port_with_width(
                format!("{prefix}_{channel}{subport}"),
                direction,
                width,
            ));
        }
    }
}

fn stream_names(name: &str, cat: ArgCategory, chan_count: Option<u32>) -> Vec<String> {
    let name = sanitize_array_name(name);
    let chan_count = chan_count.unwrap_or(1);
    if matches!(cat, ArgCategory::Istreams | ArgCategory::Ostreams) && chan_count > 1 {
        (0..chan_count).map(|idx| format!("{name}_{idx}")).collect()
    } else {
        vec![name]
    }
}

fn add_istream_ports(ports: &mut Vec<Port>, name: &str, width: u32) {
    let stream_width = width.saturating_add(1);
    ports.extend([
        port_with_width(format!("{name}_s_dout"), Direction::Input, stream_width),
        simple_port(format!("{name}_s_empty_n"), Direction::Input),
        simple_port(format!("{name}_s_read"), Direction::Output),
        port_with_width(format!("{name}_peek_dout"), Direction::Input, stream_width),
        simple_port(format!("{name}_peek_empty_n"), Direction::Input),
        simple_port(format!("{name}_peek_read"), Direction::Output),
    ]);
}

fn add_ostream_ports(ports: &mut Vec<Port>, name: &str, width: u32) {
    let stream_width = width.saturating_add(1);
    ports.extend([
        port_with_width(format!("{name}_s_din"), Direction::Output, stream_width),
        simple_port(format!("{name}_s_full_n"), Direction::Input),
        simple_port(format!("{name}_s_write"), Direction::Output),
        port_with_width(format!("{name}_peek"), Direction::Input, stream_width),
    ]);
}

fn add_addr_ostream_ports(ports: &mut Vec<Port>, name: &str) {
    ports.extend([
        port_with_width(format!("{name}_s_din"), Direction::Output, 64),
        simple_port(format!("{name}_s_full_n"), Direction::Input),
        simple_port(format!("{name}_s_write"), Direction::Output),
        port_with_width(format!("{name}_offset"), Direction::Input, 64),
    ]);
}

fn add_async_mmap_ports(ports: &mut Vec<Port>, name: &str, data_width: u32) {
    let name = sanitize_array_name(name);
    add_addr_ostream_ports(ports, &format!("{name}_read_addr"));
    add_istream_ports(ports, &format!("{name}_read_data"), data_width);
    add_addr_ostream_ports(ports, &format!("{name}_write_addr"));
    add_ostream_ports(ports, &format!("{name}_write_data"), data_width);
    add_istream_ports(ports, &format!("{name}_write_resp"), 8);
}

fn add_mmap_ports(ports: &mut Vec<Port>, name: &str, width: u32, chan_count: Option<u32>) {
    let name = sanitize_array_name(name);
    if let Some(chan_count) = chan_count {
        for idx in 0..chan_count {
            let indexed_name = format!("{name}_{idx}");
            ports.push(port_with_width(
                format!("{indexed_name}_offset"),
                Direction::Input,
                64,
            ));
            add_m_axi_ports(ports, &indexed_name, width);
        }
    } else {
        ports.push(port_with_width(
            format!("{name}_offset"),
            Direction::Input,
            64,
        ));
        add_m_axi_ports(ports, &name, width);
    }
}

/// Render the port-only Verilog module used as the starting point for a
/// `target("ignore")` custom RTL implementation.
#[must_use]
pub fn render_task_template(name: &str, task: &TaskDesign) -> String {
    let mut ports = vec![
        simple_port(HANDSHAKE_CLK, Direction::Input),
        simple_port(HANDSHAKE_RST_N, Direction::Input),
        simple_port(HANDSHAKE_START, Direction::Input),
        simple_port(HANDSHAKE_DONE, Direction::Output),
        simple_port(HANDSHAKE_IDLE, Direction::Output),
        simple_port(HANDSHAKE_READY, Direction::Output),
    ];

    for task_port in &task.ports {
        match task_port.cat {
            ArgCategory::Scalar => ports.push(port_with_width(
                sanitize_array_name(&task_port.name),
                Direction::Input,
                task_port.width,
            )),
            ArgCategory::Istream | ArgCategory::Istreams => {
                for stream in stream_names(&task_port.name, task_port.cat, task_port.chan_count) {
                    add_istream_ports(&mut ports, &stream, task_port.width);
                }
            }
            ArgCategory::Ostream | ArgCategory::Ostreams => {
                for stream in stream_names(&task_port.name, task_port.cat, task_port.chan_count) {
                    add_ostream_ports(&mut ports, &stream, task_port.width);
                }
            }
            ArgCategory::AsyncMmap => {
                add_async_mmap_ports(&mut ports, &task_port.name, task_port.width);
            }
            ArgCategory::Mmap | ArgCategory::Immap | ArgCategory::Ommap => add_mmap_ports(
                &mut ports,
                &task_port.name,
                task_port.width,
                task_port.chan_count,
            ),
        }
    }

    let ports: Vec<String> = ports.iter().map(ToString::to_string).collect();
    let mut env = minijinja::Environment::new();
    env.add_template(
        "template_module",
        include_str!("templates/template_module.v.j2"),
    )
    .expect("template parses");
    env.get_template("template_module")
        .expect("template exists")
        .render(minijinja::context! { name, ports })
        .expect("render succeeds")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(ports: &serde_json::Value) -> TaskDesign {
        serde_json::from_value(serde_json::json!({
            "level": "lower",
            "code": "",
            "target": "ignore",
            "ports": ports,
            "tasks": {},
            "fifos": {}
        }))
        .unwrap()
    }

    #[test]
    fn stream_template_matches_hls_port_shape() {
        let task = task(&serde_json::json!([
            {"cat": "istream", "name": "a", "type": "float", "width": 32},
            {"cat": "ostream", "name": "c", "type": "float", "width": 32},
            {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
        ]));
        let template = render_task_template("Add_Upper", &task);
        assert!(template.contains("module Add_Upper"));
        assert!(template.contains("input wire [32:0] a_s_dout"));
        assert!(template.contains("input wire [32:0] a_peek_dout"));
        assert!(template.contains("output wire [32:0] c_s_din"));
        assert!(template.contains("input wire [32:0] c_peek"));
        assert!(template.contains("input wire [63:0] n"));
    }

    #[test]
    fn single_channel_hmap_template_remains_indexed() {
        let task = task(&serde_json::json!([
            {"cat": "mmap", "name": "mem", "type": "float*", "width": 32,
             "chan_count": 1, "chan_size": 1024}
        ]));
        let template = render_task_template("Custom", &task);
        assert!(template.contains("input wire [63:0] mem_0_offset"));
        assert!(template.contains("output wire [63:0] m_axi_mem_0_ARADDR"));
        assert!(!template.contains(" m_axi_mem_ARADDR"));
    }

    #[test]
    fn single_channel_stream_bundle_uses_hls_base_name() {
        let task = task(&serde_json::json!([
            {"cat": "istreams", "name": "in", "type": "float", "width": 32,
             "chan_count": 1},
            {"cat": "ostreams", "name": "out", "type": "float", "width": 32,
             "chan_count": 1}
        ]));
        let template = render_task_template("Custom", &task);
        assert!(template.contains("input wire [32:0] in_s_dout"));
        assert!(template.contains("output wire [32:0] out_s_din"));
        assert!(!template.contains("in_0_s_dout"));
        assert!(!template.contains("out_0_s_din"));
    }

    #[test]
    fn async_mmap_template_uses_fifo_channels() {
        let task = task(&serde_json::json!([
            {"cat": "async_mmap", "name": "mem", "type": "float*", "width": 32}
        ]));
        let template = render_task_template("Custom", &task);
        assert!(template.contains("output wire [63:0] mem_read_addr_s_din"));
        assert!(template.contains("input wire [63:0] mem_read_addr_offset"));
        assert!(template.contains("input wire [32:0] mem_read_data_s_dout"));
        assert!(template.contains("output wire [32:0] mem_write_data_s_din"));
        assert!(template.contains("input wire [8:0] mem_write_resp_s_dout"));
        assert!(!template.contains("m_axi_mem_ARADDR"));
    }
}
