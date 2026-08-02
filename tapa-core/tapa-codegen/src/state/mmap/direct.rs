//! Direct M-AXI interface catalog.
//!
//! [`DirectMmapInterface`] describes a child M-AXI interface connected
//! directly to a top-level mmap port: a read-only projection of the topology
//! validated against the attached child RTL, including per-port width
//! resolution for the protocol channels.

use tapa_ir::task::TaskLevel;
use tapa_ir::{ArgCategory, AxiChannelWidths, AxiEndpoint, Design};
use tapa_protocol::{
    axi_subport_from_suffix, axi_subport_width, PortDir, AXI_ADDR_WIDTH, AXI_ID_WIDTH,
    M_AXI_PREFIX, M_AXI_SUFFIXES_BY_CHANNEL, M_AXI_SUFFIXES_COMPACT,
};
use tapa_rtl::expression::{expression_as_u32, expression_source, Expression};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::port::{Direction, Port as RtlPort};
use tapa_rtl::VerilogModule;

use crate::error::CodegenError;
use crate::state::mmap::{MMapConnection, MMapSlave};
use crate::state::rtl_state::TopologyWithRtl;

/// One child M-AXI interface connected directly to a top-level mmap port.
///
/// This is a read-only projection of the topology and attached child RTL. It
/// deliberately contains no mutable RTL or physical-device state, so the
/// floorplanner can consume it without depending on code-generation details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectMmapInterface {
    /// Canonical flattened child endpoint.
    pub endpoint: AxiEndpoint,
    /// AXI data width in bits, from topology metadata.
    pub data_width: u32,
    /// AXI address width in bits.
    pub addr_width: u32,
    /// AXI ID width in bits, resolved from all four child RTL ID ports.
    pub id_width: u32,
    /// Physical widths of the independently routed channels. Zero-valued
    /// channels are pruned by a read-only or write-only async mmap bridge.
    pub channel_widths: AxiChannelWidths,
    /// Generated FIFO-to-AXI bridge hierarchy, or `None` when the child
    /// exposes a complete compact M-AXI interface itself.
    pub bridge_instance: Option<String>,
}

impl TopologyWithRtl {
    /// Catalog child M-AXI interfaces connected directly to ports of
    /// `task_name`.
    ///
    /// The first floorplanned implementation intentionally accepts only the
    /// Shared mmap and hmap interfaces are rejected. A FIFO-style async mmap
    /// is represented by its generated bridge and only the AXI directions
    /// that survive conservative RTL tie-off analysis.
    pub fn direct_mmap_interfaces(
        &self,
        task_name: &str,
    ) -> Result<Vec<DirectMmapInterface>, CodegenError> {
        let task = self
            .design
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;
        let connections = self.aggregate_mmap_connections(task_name)?;
        connections
            .values()
            .map(|connection| self.direct_mmap_interface(task_name, task, connection))
            .collect()
    }

    fn direct_mmap_interface(
        &self,
        task_name: &str,
        task: &tapa_ir::Task,
        connection: &MMapConnection,
    ) -> Result<DirectMmapInterface, CodegenError> {
        let qualified_port = format!("{task_name}.{}", connection.arg_name);
        validate_plain_parent_mmap(task, connection, &qualified_port)?;
        let (slave, instance_index, instance) =
            direct_mmap_child_instance(task, connection, &qualified_port)?;
        let child_category =
            validate_direct_child_mmap(&self.design, instance, slave, &qualified_port)?;
        if connection.data_width == 0 || !connection.data_width.is_multiple_of(8) {
            return Err(invalid_direct_mmap(
                &qualified_port,
                &format!(
                    "has data width {}, expected a nonzero multiple of 8 bits",
                    connection.data_width
                ),
            ));
        }

        let module = self.module_map.get(&slave.task).ok_or_else(|| {
            invalid_direct_mmap(
                &qualified_port,
                &format!("has no RTL module attached for child task '{}'", slave.task),
            )
        })?;
        let (id_width, channel_widths, bridge_instance) = catalog_direct_mmap_rtl(
            &module.inner,
            child_category,
            slave,
            &qualified_port,
            connection.data_width,
            &connection.arg_name,
        )?;

        Ok(DirectMmapInterface {
            endpoint: AxiEndpoint {
                instance: instance
                    .canonical_name(&slave.task, instance_index)
                    .into_owned(),
                port: slave.port.clone(),
                top_port: connection.arg_name.clone(),
            },
            data_width: connection.data_width,
            addr_width: AXI_ADDR_WIDTH,
            id_width,
            channel_widths,
            bridge_instance,
        })
    }
}

fn catalog_direct_mmap_rtl(
    module: &VerilogModule,
    child_category: ArgCategory,
    slave: &MMapSlave,
    interface: &str,
    data_width: u32,
    top_port: &str,
) -> Result<(u32, AxiChannelWidths, Option<String>), CodegenError> {
    if child_category == ArgCategory::Mmap
        || crate::passes::async_mmap::has_direct_m_axi_ports(module, &slave.port)
    {
        let rtl_prefix = format!("{M_AXI_PREFIX}{}", sanitize_array_name(&slave.port));
        let id_width = validate_compact_m_axi_ports(module, interface, &rtl_prefix, data_width)?;
        return Ok((
            id_width,
            direct_m_axi_channel_widths(data_width, id_width),
            None,
        ));
    }
    debug_assert_eq!(
        child_category,
        ArgCategory::AsyncMmap,
        "direct child validator only permits mmap categories"
    );

    let tags = crate::passes::async_mmap::active_tags(module, &slave.port);
    if tags.is_empty() {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "has async mmap child '{}.{}' with neither FIFO-style async ports nor a compact M-AXI interface",
                slave.task, slave.port
            ),
        ));
    }
    let enabled = crate::passes::async_mmap::enabled_axi_directions(module, &slave.port, &tags);
    let id_width = AXI_ID_WIDTH;
    let mut widths = direct_m_axi_channel_widths(data_width, id_width);
    if !enabled.read {
        widths.read_address = 0;
        widths.read_data = 0;
    }
    if !enabled.write {
        widths.write_address = 0;
        widths.write_data = 0;
        widths.write_response = 0;
    }
    Ok((
        id_width,
        widths,
        Some(tapa_ir::async_mmap_bridge_instance_name(top_port)),
    ))
}

fn validate_plain_parent_mmap(
    task: &tapa_ir::Task,
    connection: &MMapConnection,
    interface: &str,
) -> Result<(), CodegenError> {
    if connection.chan_count.is_some() || connection.chan_size.is_some() {
        return Err(invalid_direct_mmap(
            interface,
            "is an hmap; channelized memory interfaces are not supported",
        ));
    }
    let parent_port = task
        .ports
        .iter()
        .find(|port| port.name == connection.arg_name)
        .ok_or_else(|| invalid_direct_mmap(interface, "has no corresponding parent task port"))?;
    validate_plain_mmap_category(parent_port.cat, interface, "parent port")
}

fn direct_mmap_child_instance<'task, 'connection>(
    task: &'task tapa_ir::Task,
    connection: &'connection MMapConnection,
    interface: &str,
) -> Result<(&'connection MMapSlave, usize, &'task tapa_ir::TaskInstance), CodegenError> {
    let [slave] = connection.slaves.as_slice() else {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "is shared by {} child ports; shared memory interfaces are not supported",
                connection.slaves.len()
            ),
        ));
    };
    let instance_index = usize::try_from(slave.inst_idx).map_err(|_| {
        invalid_direct_mmap(
            interface,
            &format!("has an invalid child instance index {}", slave.inst_idx),
        )
    })?;
    let child_instances = task.tasks.get(&slave.task).ok_or_else(|| {
        invalid_direct_mmap(
            interface,
            &format!("references missing child task definition '{}'", slave.task),
        )
    })?;
    let instance = child_instances.get(instance_index).ok_or_else(|| {
        invalid_direct_mmap(
            interface,
            &format!(
                "references missing child instance '{}[{}]'",
                slave.task, slave.inst_idx
            ),
        )
    })?;
    Ok((slave, instance_index, instance))
}

fn validate_direct_child_mmap(
    design: &Design,
    instance: &tapa_ir::TaskInstance,
    slave: &MMapSlave,
    interface: &str,
) -> Result<ArgCategory, CodegenError> {
    let binding = instance.args.get(&slave.port).ok_or_else(|| {
        invalid_direct_mmap(
            interface,
            &format!(
                "references missing child binding '{}.{}'",
                slave.task, slave.port
            ),
        )
    })?;
    let child_location = format!("child port '{}.{}'", slave.task, slave.port);
    if !binding.cat.is_direct_mmap() {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "has category '{}' at binding {child_location}, expected mmap or async_mmap",
                binding.cat.as_str()
            ),
        ));
    }

    let child_task = design.tasks.get(&slave.task).ok_or_else(|| {
        invalid_direct_mmap(
            interface,
            &format!("references missing child task definition '{}'", slave.task),
        )
    })?;
    if child_task.level != TaskLevel::Lower {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "reaches upper-level child task '{}'; catalog a flattened design",
                slave.task
            ),
        ));
    }
    let child_port = child_task
        .ports
        .iter()
        .find(|port| port.name == slave.port)
        .ok_or_else(|| {
            invalid_direct_mmap(
                interface,
                &format!(
                    "has no child port metadata for '{}.{}'",
                    slave.task, slave.port
                ),
            )
        })?;
    if child_port.cat != binding.cat {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "has category '{}' at binding {child_location} but '{}' in child port metadata",
                binding.cat.as_str(),
                child_port.cat.as_str()
            ),
        ));
    }
    Ok(child_port.cat)
}

fn validate_plain_mmap_category(
    category: ArgCategory,
    interface: &str,
    location: &str,
) -> Result<(), CodegenError> {
    if category == ArgCategory::AsyncMmap {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "is connected to async mmap {location}; async memory interfaces are not supported"
            ),
        ));
    }
    if category != ArgCategory::Mmap {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "has category '{}' at {location}, expected plain mmap",
                category.as_str()
            ),
        ));
    }
    Ok(())
}

fn invalid_direct_mmap(interface: &str, reason: &str) -> CodegenError {
    CodegenError::InvalidMmapConnection(format!("direct M-AXI interface '{interface}' {reason}"))
}

fn direct_m_axi_channel_widths(data_width: u32, id_width: u32) -> AxiChannelWidths {
    let physical_width = |channel: &str| {
        M_AXI_SUFFIXES_BY_CHANNEL[channel]
            .ports
            .iter()
            .filter(|suffix| M_AXI_SUFFIXES_COMPACT.contains(suffix))
            .map(|suffix| {
                axi_subport_width(
                    axi_subport_from_suffix(suffix),
                    data_width,
                    AXI_ADDR_WIDTH,
                    id_width,
                )
            })
            .sum()
    };

    AxiChannelWidths {
        read_address: physical_width("AR"),
        read_data: physical_width("R"),
        write_address: physical_width("AW"),
        write_data: physical_width("W"),
        write_response: physical_width("B"),
    }
}

fn validate_compact_m_axi_ports(
    module: &VerilogModule,
    interface: &str,
    rtl_prefix: &str,
    data_width: u32,
) -> Result<u32, CodegenError> {
    let mut id_width = None;

    for suffix in M_AXI_SUFFIXES_COMPACT {
        let port_name = format!("{rtl_prefix}{suffix}");
        let port = module.find_port(&port_name).ok_or_else(|| {
            invalid_direct_mmap(
                interface,
                &format!(
                    "is missing required child RTL port '{}.{port_name}'",
                    module.name
                ),
            )
        })?;
        let expected_direction = m_axi_port_direction(suffix).ok_or_else(|| {
            invalid_direct_mmap(
                interface,
                &format!("has unknown protocol suffix '{suffix}'"),
            )
        })?;
        if port.direction != expected_direction {
            return Err(invalid_direct_mmap(
                interface,
                &format!(
                    "has child RTL port '{}.{port_name}' with direction {:?}, expected {:?}",
                    module.name, port.direction, expected_direction
                ),
            ));
        }

        let subport = axi_subport_from_suffix(suffix);
        let resolved_width = resolve_rtl_port_width(module, port);
        if subport == "ID" {
            let width = resolved_width.ok_or_else(|| {
                invalid_direct_mmap(
                    interface,
                    &format!(
                        "cannot resolve ID width of child RTL port '{}.{port_name}'{}",
                        module.name,
                        render_port_width(port)
                    ),
                )
            })?;
            if let Some(previous) = id_width {
                if width != previous {
                    return Err(invalid_direct_mmap(
                        interface,
                        &format!(
                            "has inconsistent child RTL ID widths: '{}.{port_name}' is {width} \
                             bits, expected {previous} bits",
                            module.name
                        ),
                    ));
                }
            } else {
                id_width = Some(width);
            }
        }
    }

    let id_width = id_width.ok_or_else(|| {
        invalid_direct_mmap(
            interface,
            "has no child RTL ID ports from which to derive ID width",
        )
    })?;

    // Literal or simply parameterized widths are cheap to verify. More complex
    // non-ID expressions remain topology-authoritative; only ID widths must be
    // resolved because they are not represented in the topology.
    for suffix in M_AXI_SUFFIXES_COMPACT {
        let port_name = format!("{rtl_prefix}{suffix}");
        let port = module
            .find_port(&port_name)
            .expect("compact port presence was validated above");
        let Some(actual_width) = resolve_rtl_port_width(module, port) else {
            continue;
        };
        let expected_width = axi_subport_width(
            axi_subport_from_suffix(suffix),
            data_width,
            AXI_ADDR_WIDTH,
            id_width,
        );
        if actual_width != expected_width {
            return Err(invalid_direct_mmap(
                interface,
                &format!(
                    "has child RTL port '{}.{port_name}' width {actual_width}, expected \
                     {expected_width}",
                    module.name
                ),
            ));
        }
    }

    Ok(id_width)
}

fn m_axi_port_direction(suffix: &str) -> Option<Direction> {
    tapa_protocol::m_axi_port_direction(suffix).map(|direction| match direction {
        PortDir::Input => Direction::Input,
        PortDir::Output => Direction::Output,
    })
}

fn render_port_width(port: &RtlPort) -> String {
    port.width.as_ref().map_or_else(String::new, |width| {
        format!(
            " [{}:{}]",
            expression_source(&width.msb),
            expression_source(&width.lsb)
        )
    })
}

fn resolve_rtl_port_width(module: &VerilogModule, port: &RtlPort) -> Option<u32> {
    let Some(width) = &port.width else {
        return Some(1);
    };
    let msb = resolve_width_endpoint(module, &width.msb)?;
    let lsb = resolve_width_endpoint(module, &width.lsb)?;
    msb.abs_diff(lsb).checked_add(1)
}

fn resolve_width_endpoint(module: &VerilogModule, expression: &Expression) -> Option<u32> {
    if let Some(value) = expression_as_u32(expression) {
        return Some(value);
    }
    let source = expression_source(expression).replace(' ', "");
    source.strip_suffix("-1").map_or_else(
        || resolve_parameter_default(module, &source),
        |parameter| resolve_parameter_default(module, parameter)?.checked_sub(1),
    )
}

fn resolve_parameter_default(module: &VerilogModule, name: &str) -> Option<u32> {
    let parameter = module
        .parameters
        .iter()
        .find(|parameter| parameter.name == name)?;
    expression_as_u32(&parameter.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_mmap_program(
        instance_name: Option<&str>,
        instance_count: usize,
        binding_category: &str,
        channelized: bool,
    ) -> Design {
        let mut parent_port = serde_json::json!({
            "cat": "mmap",
            "name": "elems",
            "type": "int*",
            "width": 32
        });
        let mut child_port = serde_json::json!({
            "cat": binding_category,
            "name": "data",
            "type": "int*",
            "width": 32
        });
        if channelized {
            for port in [&mut parent_port, &mut child_port] {
                port["chan_count"] = serde_json::json!(2);
                port["chan_size"] = serde_json::json!(1024);
            }
        }
        let instances: Vec<_> = (0..instance_count)
            .map(|index| {
                let mut instance = serde_json::json!({
                    "args": {
                        "data": {"arg": "elems", "cat": binding_category}
                    }
                });
                if let Some(name) = instance_name {
                    instance["name"] = serde_json::json!(format!("{name}_{index}"));
                }
                instance
            })
            .collect();
        crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [parent_port],
                    "tasks": {"leaf": instances},
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [child_port],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }))
    }

    fn compact_m_axi_module(data_width: u32, id_width: u32) -> VerilogModule {
        let names = M_AXI_SUFFIXES_COMPACT
            .iter()
            .map(|suffix| format!("  m_axi_data{suffix}"))
            .collect::<Vec<_>>()
            .join(",\n");
        let declarations = M_AXI_SUFFIXES_COMPACT
            .iter()
            .map(|suffix| {
                let direction = match m_axi_port_direction(suffix).expect("known AXI suffix") {
                    Direction::Input => "input",
                    Direction::Output => "output",
                    Direction::Inout => unreachable!("M-AXI ports are never inout"),
                };
                let subport = axi_subport_from_suffix(suffix);
                let width = axi_subport_width(subport, data_width, AXI_ADDR_WIDTH, id_width);
                let width_decl = if subport == "ID" {
                    "[C_M_AXI_DATA_ID_WIDTH - 1:0] ".to_owned()
                } else if width > 1 {
                    format!("[{}:0] ", width - 1)
                } else {
                    String::new()
                };
                format!("{direction} wire {width_decl}m_axi_data{suffix};")
            })
            .collect::<Vec<_>>()
            .join("\n");
        VerilogModule::parse(&format!(
            "module leaf(\n{names}\n);\n\
             parameter C_M_AXI_DATA_ID_WIDTH = {id_width};\n\
             {declarations}\n\
             endmodule"
        ))
        .expect("valid compact M-AXI fixture")
    }

    fn plain_direct_mmap_state(instance_name: Option<&str>) -> TopologyWithRtl {
        let mut state = TopologyWithRtl::new(direct_mmap_program(instance_name, 1, "mmap", false));
        state
            .attach_module("leaf", compact_m_axi_module(32, 3))
            .unwrap();
        state
    }

    #[test]
    fn direct_mmap_catalog_resolves_symbolic_id_and_physical_widths() {
        let state = plain_direct_mmap_state(Some("reader"));

        let interfaces = state.direct_mmap_interfaces("top").unwrap();

        assert_eq!(interfaces.len(), 1);
        assert_eq!(
            interfaces[0],
            DirectMmapInterface {
                endpoint: AxiEndpoint {
                    instance: "reader_0".to_owned(),
                    port: "data".to_owned(),
                    top_port: "elems".to_owned(),
                },
                data_width: 32,
                addr_width: 64,
                id_width: 3,
                channel_widths: AxiChannelWidths {
                    read_address: 82,
                    read_data: 40,
                    write_address: 82,
                    write_data: 39,
                    write_response: 7,
                },
                bridge_instance: None,
            }
        );
    }

    #[test]
    fn direct_mmap_catalog_uses_canonical_instance_name() {
        let state = plain_direct_mmap_state(None);

        let interfaces = state.direct_mmap_interfaces("top").unwrap();

        assert_eq!(interfaces[0].endpoint.instance, "leaf_0");
    }

    #[test]
    fn direct_mmap_catalog_rejects_shared_and_hmap_interfaces() {
        let cases = [
            (
                direct_mmap_program(None, 2, "mmap", false),
                "shared by 2 child ports",
            ),
            (direct_mmap_program(None, 1, "mmap", true), "is an hmap"),
        ];

        for (design, expected) in cases {
            let state = TopologyWithRtl::new(design);
            let error = state
                .direct_mmap_interfaces("top")
                .expect_err("unsupported memory topology must be rejected");
            assert!(
                error.to_string().contains(expected),
                "expected '{expected}', got: {error}"
            );
        }
    }

    fn fifo_style_async_module(write_tied_off: bool) -> VerilogModule {
        let write_activity = if write_tied_off { "1'b0" } else { "live" };
        VerilogModule::parse(&format!(
            "module leaf(\n\
             output wire data_read_addr_s_write,\n\
             output wire data_read_data_s_read,\n\
             output wire data_write_addr_s_write,\n\
             output wire data_write_data_s_write,\n\
             output wire data_write_resp_s_read\n\
             );\n\
             assign data_read_addr_s_write = live;\n\
             assign data_read_data_s_read = live;\n\
             assign data_write_addr_s_write = {write_activity};\n\
             assign data_write_data_s_write = {write_activity};\n\
             assign data_write_resp_s_read = {write_activity};\n\
             endmodule"
        ))
        .expect("valid FIFO-style async mmap fixture")
    }

    #[test]
    fn direct_mmap_catalog_models_read_only_async_bridge() {
        let mut state =
            TopologyWithRtl::new(direct_mmap_program(Some("reader"), 1, "async_mmap", false));
        state
            .attach_module("leaf", fifo_style_async_module(true))
            .unwrap();

        let interfaces = state.direct_mmap_interfaces("top").unwrap();

        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].id_width, AXI_ID_WIDTH);
        assert_eq!(interfaces[0].addr_width, AXI_ADDR_WIDTH);
        assert_eq!(
            interfaces[0].channel_widths,
            AxiChannelWidths {
                read_address: 80,
                read_data: 38,
                write_address: 0,
                write_data: 0,
                write_response: 0,
            }
        );
        assert_eq!(
            interfaces[0].bridge_instance.as_deref(),
            Some("elems__m_axi")
        );
    }

    #[test]
    fn direct_mmap_catalog_preserves_complete_direct_axi_async_child() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "async_mmap", false));
        state
            .attach_module("leaf", compact_m_axi_module(32, 3))
            .unwrap();

        let interface = state.direct_mmap_interfaces("top").unwrap().remove(0);

        assert_eq!(interface.id_width, 3);
        assert!(interface
            .channel_widths
            .channels()
            .into_iter()
            .all(|(_, width)| width != 0));
        assert_eq!(interface.bridge_instance, None);
    }

    #[test]
    fn direct_mmap_catalog_rejects_partial_direct_axi_async_child() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "async_mmap", false));
        state
            .attach_module(
                "leaf",
                VerilogModule::parse(
                    "module leaf(input wire [63:0] data_offset, output wire [63:0] m_axi_data_ARADDR); endmodule",
                )
                .unwrap(),
            )
            .unwrap();

        let error = state.direct_mmap_interfaces("top").unwrap_err();
        assert!(error
            .to_string()
            .contains("missing required child RTL port"));
    }

    #[test]
    fn direct_mmap_catalog_rejects_async_without_fifo_or_axi_shape() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "async_mmap", false));
        state
            .attach_module(
                "leaf",
                VerilogModule::parse("module leaf(input wire ap_clk); endmodule").unwrap(),
            )
            .unwrap();

        let error = state.direct_mmap_interfaces("top").unwrap_err();
        assert!(error.to_string().contains("neither FIFO-style async ports"));
    }

    #[test]
    fn direct_mmap_catalog_requires_a_flattened_child() {
        let mut design = direct_mmap_program(None, 1, "mmap", false);
        design.tasks.get_mut("leaf").expect("leaf").level = TaskLevel::Upper;
        let state = TopologyWithRtl::new(design);

        let error = state
            .direct_mmap_interfaces("top")
            .expect_err("an upper child can hide unmodeled memory infrastructure");

        assert!(error.to_string().contains("catalog a flattened design"));
    }

    #[test]
    fn direct_mmap_catalog_requires_every_compact_port() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "mmap", false));
        let mut module = compact_m_axi_module(32, 3);
        module.ports.retain(|port| port.name != "m_axi_data_WVALID");
        state.attach_module("leaf", module).unwrap();

        let error = state
            .direct_mmap_interfaces("top")
            .expect_err("missing compact port must be rejected");
        assert!(error
            .to_string()
            .contains("missing required child RTL port"));
        assert!(error.to_string().contains("leaf.m_axi_data_WVALID"));
    }

    #[test]
    fn direct_mmap_catalog_validates_master_side_directions() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "mmap", false));
        let mut module = compact_m_axi_module(32, 3);
        module
            .ports
            .iter_mut()
            .find(|port| port.name == "m_axi_data_ARREADY")
            .expect("fixture ARREADY")
            .direction = Direction::Output;
        state.attach_module("leaf", module).unwrap();

        let error = state
            .direct_mmap_interfaces("top")
            .expect_err("wrong child port direction must be rejected");
        assert!(error.to_string().contains("m_axi_data_ARREADY"));
        assert!(error.to_string().contains("expected Input"));
    }

    #[test]
    fn direct_mmap_catalog_rejects_unresolved_id_width() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "mmap", false));
        let mut module = compact_m_axi_module(32, 3);
        module.parameters.clear();
        state.attach_module("leaf", module).unwrap();

        let error = state
            .direct_mmap_interfaces("top")
            .expect_err("unresolved child ID width must be rejected");
        assert!(error.to_string().contains("cannot resolve ID width"));
        assert!(error.to_string().contains("m_axi_data_ARID"));
    }

    #[test]
    fn direct_mmap_catalog_rejects_inconsistent_id_widths() {
        let mut state = TopologyWithRtl::new(direct_mmap_program(None, 1, "mmap", false));
        let mut module = compact_m_axi_module(32, 3);
        module
            .ports
            .iter_mut()
            .find(|port| port.name == "m_axi_data_RID")
            .expect("fixture RID")
            .width = Some(tapa_rtl::port::Width {
            msb: tapa_rtl::expression::tokenize_expression("1"),
            lsb: tapa_rtl::expression::tokenize_expression("0"),
        });
        state.attach_module("leaf", module).unwrap();

        let error = state
            .direct_mmap_interfaces("top")
            .expect_err("inconsistent child ID widths must be rejected");
        assert!(error
            .to_string()
            .contains("inconsistent child RTL ID widths"));
        assert!(error.to_string().contains("m_axi_data_RID"));
    }
}
