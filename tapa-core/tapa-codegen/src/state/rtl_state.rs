//! Enriched topology + RTL state for code generation.
//!
//! `TopologyWithRtl` wraps a `Design` with attached Verilog modules
//! parsed from HLS output, plus FSM modules created during codegen.

use std::collections::BTreeMap;

use tapa_ir::task::TaskLevel;
use tapa_ir::Port as IrPort;
use tapa_ir::{ArgCategory, AxiChannelWidths, AxiEndpoint, Design, FloorplanResult};
use tapa_protocol::{
    axi_subport_from_suffix, axi_subport_width, PortDir, AXI_ADDR_WIDTH, AXI_ID_WIDTH,
    M_AXI_PREFIX, M_AXI_SUFFIXES_BY_CHANNEL, M_AXI_SUFFIXES_COMPACT,
};
use tapa_rtl::expression::{expression_as_u32, expression_source, Expression};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::MutableModule;
use tapa_rtl::port::{Direction, Port as RtlPort};
use tapa_rtl::VerilogModule;

use crate::error::CodegenError;

fn validate_mmap_channel_geometry(
    parent_task_name: &str,
    parent_port_name: &str,
    parent: &IrPort,
    child_task_name: &str,
    child_port_name: &str,
    child: &IrPort,
) -> Result<(), CodegenError> {
    match (parent.chan_count, child.chan_count) {
        (Some(parent_count), Some(child_count)) if parent_count != child_count => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap channel-count mismatch: '{parent_task_name}.{parent_port_name}' declares \
                 {parent_count}, but '{child_task_name}.{child_port_name}' declares {child_count}",
            )));
        }
        _ => {}
    }
    match (parent.chan_size, child.chan_size) {
        (Some(parent_size), Some(child_size)) if parent_size != child_size => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap channel-size mismatch: '{parent_task_name}.{parent_port_name}' declares \
                 {parent_size}, but '{child_task_name}.{child_port_name}' declares {child_size}",
            )));
        }
        _ => {}
    }
    Ok(())
}

fn merge_mmap_port_metadata(
    parent_task_name: &str,
    parent_port_name: &str,
    parent: Option<&IrPort>,
    child_task_name: &str,
    child_port_name: &str,
    child: Option<&IrPort>,
) -> Result<(u32, Option<u32>, Option<u32>), CodegenError> {
    if let (Some(parent), Some(child)) = (parent, child) {
        if parent.width != child.width {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "mmap width mismatch: '{parent_task_name}.{parent_port_name}' is {} bits, but \
                 '{child_task_name}.{child_port_name}' is {} bits",
                parent.width, child.width,
            )));
        }
        validate_mmap_channel_geometry(
            parent_task_name,
            parent_port_name,
            parent,
            child_task_name,
            child_port_name,
            child,
        )?;
    }

    let data_width = parent.or(child).map(|port| port.width).ok_or_else(|| {
        CodegenError::InvalidMmapConnection(format!(
            "no port named '{parent_port_name}' on task '{parent_task_name}' or \
             '{child_port_name}' on child '{child_task_name}' to derive the mmap data width from",
        ))
    })?;
    let chan_count = parent
        .and_then(|port| port.chan_count)
        .or_else(|| child.and_then(|port| port.chan_count));
    let chan_size = parent
        .and_then(|port| port.chan_size)
        .or_else(|| child.and_then(|port| port.chan_size));
    Ok((data_width, chan_count, chan_size))
}

/// One crossbar slave: a child port bound to a shared mmap argument.
#[derive(Debug, Clone)]
pub struct MMapSlave {
    /// Child task name.
    pub task: String,
    /// Instance index within the child task's instantiation list.
    pub inst_idx: u32,
    /// Child-side port name.
    pub port: String,
    /// Aggregated AXI thread count this port presents: a leaf child
    /// contributes 1; an upper child that internally shares the mmap
    /// contributes its own aggregated total, so the crossbar's
    /// `S*_THREADS` can track every outstanding ID.
    pub threads: u32,
    /// AXI ID width the child port presents (from its RTL module and
    /// any nested aggregation).
    pub id_width: u32,
}

/// Aggregated M-AXI memory-mapped connection info for a single argument.
#[derive(Debug, Clone)]
pub struct MMapConnection {
    /// Argument name.
    pub arg_name: String,
    /// The child ports sharing this argument, one crossbar slave each.
    pub slaves: Vec<MMapSlave>,
    /// Channel count; `None` for a plain (non-hmap) mmap. `Some(1)`
    /// is a single-channel hmap, which still gets a crossbar.
    pub chan_count: Option<u32>,
    /// Channel size in elements; `None` for a plain mmap.
    pub chan_size: Option<u32>,
    /// Data width in bits.
    pub data_width: u32,
}

impl MMapConnection {
    /// Parent-facing AXI ID width: the widest child ID plus enough
    /// routing bits to identify the originating slave.
    #[must_use]
    pub fn id_width(&self) -> u32 {
        let widest_child = self.slaves.iter().map(|s| s.id_width).max().unwrap_or(1);
        id_width_for_child_threads(widest_child, self.thread_count())
    }

    /// Crossbar slave count.
    #[must_use]
    pub fn thread_count(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation, reason = "slave count fits in u32")]
        {
            self.slaves.len() as u32
        }
    }

    /// Upstream channel count; a plain mmap behaves as one channel.
    #[must_use]
    pub fn channel_count(&self) -> u32 {
        self.chan_count.unwrap_or(1)
    }

    /// Sum of the per-slave aggregated thread counts.
    #[must_use]
    pub fn total_threads(&self) -> u32 {
        self.slaves.iter().map(|s| s.threads).sum()
    }
}

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

/// Enriched state combining topology with RTL modules.
pub struct TopologyWithRtl {
    /// The design model.
    pub design: Design,
    /// The floorplan, when the design has been floorplanned. Its presence
    /// switches codegen onto the pipelined path (Head/Body/Tail cells on
    /// cross-slot streams and matching region constraints).
    pub floorplan: Option<FloorplanResult>,
    /// Parsed HLS Verilog modules, keyed by task name.
    pub module_map: BTreeMap<String, MutableModule>,
    /// FSM modules for upper-level tasks, keyed by task name.
    pub fsm_modules: BTreeMap<String, MutableModule>,
    /// Generated auxiliary RTL files, keyed by file path.
    pub generated_files: BTreeMap<String, String>,
    /// Port-only custom RTL templates, keyed by `<task>.v`.
    pub template_files: BTreeMap<String, String>,
}

impl TopologyWithRtl {
    /// Create a new `TopologyWithRtl` from a `Design`.
    pub fn new(design: Design) -> Self {
        Self {
            design,
            floorplan: None,
            module_map: BTreeMap::new(),
            fsm_modules: BTreeMap::new(),
            generated_files: BTreeMap::new(),
            template_files: BTreeMap::new(),
        }
    }

    /// Whether codegen can emit the distributed controller for the top task.
    ///
    /// This deliberately checks the same upper-task boundary used by
    /// [`crate::generate_rtl`]. Callers preparing a floorplan can use it to
    /// avoid requesting controller hierarchy that codegen would not create.
    #[must_use]
    pub fn supports_distributed_control(&self) -> bool {
        self.design.tasks.get(&self.design.top).is_some_and(|task| {
            task.level == TaskLevel::Upper
                && task.synth != tapa_ir::SynthTarget::Ignore
                && !task.tasks.is_empty()
        }) && self.module_map.contains_key(&self.design.top)
    }

    /// Whether the generated top will instantiate the AXI-Lite control block.
    ///
    /// This is the single read-only predicate shared with the floorplanner.
    /// The pipeline stages it at prepare time for the `s_axi` pass; the
    /// distributed-control plan builder and `tapa-cli` read it directly.
    #[must_use]
    pub fn top_instantiates_control_s_axi(&self) -> bool {
        self.design.tasks.get(&self.design.top).is_some_and(|task| {
            task.level == TaskLevel::Upper && task.synth != tapa_ir::SynthTarget::Ignore
        }) && self.module_map.get(&self.design.top).is_some_and(|module| {
            module
                .inner
                .ports
                .iter()
                .any(|port| port.name == "s_axi_control_AWVALID")
        })
    }

    /// Attach a parsed HLS Verilog module to a task.
    ///
    /// Rejects nonexistent task names and duplicate attachments.
    pub fn attach_module(
        &mut self,
        task_name: &str,
        module: VerilogModule,
    ) -> Result<(), CodegenError> {
        if !self.design.tasks.contains_key(task_name) {
            return Err(CodegenError::TaskNotFound(task_name.to_owned()));
        }
        if self.module_map.contains_key(task_name) {
            return Err(CodegenError::ModuleAlreadyAttached(task_name.to_owned()));
        }
        self.module_map
            .insert(task_name.to_owned(), MutableModule::from_parsed(module));
        Ok(())
    }

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

    /// Aggregate M-AXI `MMapConnection` data from topology instances.
    ///
    /// For each upper-level task, collects all mmap/`async_mmap` arguments
    /// from child instances and groups them by argument name.
    pub fn aggregate_mmap_connections(
        &self,
        task_name: &str,
    ) -> Result<BTreeMap<String, MMapConnection>, CodegenError> {
        self.aggregate_mmap_connections_cached(task_name, &mut BTreeMap::new())
    }

    /// Recursive worker for [`Self::aggregate_mmap_connections`]. The
    /// cache holds each upper task's finished aggregation so nested
    /// hierarchies are aggregated once per task, not once per lookup.
    fn aggregate_mmap_connections_cached(
        &self,
        task_name: &str,
        cache: &mut BTreeMap<String, BTreeMap<String, MMapConnection>>,
    ) -> Result<BTreeMap<String, MMapConnection>, CodegenError> {
        let task = self
            .design
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;

        let mut connections: BTreeMap<String, MMapConnection> = BTreeMap::new();

        for (child_task_name, instances) in &task.tasks {
            for (inst_idx, instance) in instances.iter().enumerate() {
                for (child_port_name, arg) in &instance.args {
                    let is_mmap = arg.cat.is_direct_mmap();
                    if !is_mmap {
                        continue;
                    }

                    // Look up child port metadata using the child's port name
                    let child_task = self.design.tasks.get(child_task_name.as_str());
                    let port = child_task
                        .and_then(|t| t.ports.iter().find(|p| p.name == *child_port_name));
                    let (child_id_width, child_threads) =
                        self.child_mmap_summary(child_task_name, child_port_name, cache)?;

                    // Group by parent scope arg name (arg.arg), not child port name
                    let parent_arg_name = &arg.arg;
                    let parent_port = task.ports.iter().find(|p| p.name == *parent_arg_name);

                    let (data_width, chan_count, chan_size) = merge_mmap_port_metadata(
                        task_name,
                        parent_arg_name,
                        parent_port,
                        child_task_name,
                        child_port_name,
                        port,
                    )?;
                    let conn = connections
                        .entry(parent_arg_name.clone())
                        .or_insert_with(|| MMapConnection {
                            arg_name: parent_arg_name.clone(),
                            slaves: Vec::new(),
                            chan_count,
                            chan_size,
                            data_width,
                        });
                    if conn.data_width != data_width {
                        return Err(CodegenError::InvalidMmapConnection(format!(
                            "mmap argument '{task_name}.{parent_arg_name}' has conflicting data \
                             widths: {} vs {data_width} at '{child_task_name}.{child_port_name}'",
                            conn.data_width
                        )));
                    }
                    if conn.chan_count != chan_count || conn.chan_size != chan_size {
                        return Err(CodegenError::InvalidMmapConnection(format!(
                            "mmap argument '{task_name}.{parent_arg_name}' has conflicting \
                             channel shapes: ({:?}, {:?}) vs ({chan_count:?}, {chan_size:?}) \
                             at '{child_task_name}.{child_port_name}'",
                            conn.chan_count, conn.chan_size
                        )));
                    }
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "instance index fits in u32"
                    )]
                    let idx = inst_idx as u32;
                    conn.slaves.push(MMapSlave {
                        task: child_task_name.clone(),
                        inst_idx: idx,
                        port: child_port_name.clone(),
                        threads: child_threads,
                        id_width: child_id_width,
                    });
                }
            }
        }

        // An hmap whose single child port internally shares the mmap
        // would need per-channel routing of an already-multiplexed ID
        // stream, which the crossbar wrapper does not implement.
        for conn in connections.values() {
            let total_threads = conn.total_threads();
            if let [slave] = conn.slaves.as_slice() {
                if conn.chan_count.is_some() && total_threads > 1 {
                    return Err(CodegenError::InvalidMmapConnection(format!(
                        "hmap argument '{}' is driven only by '{}.{}', which \
                         internally shares the mmap ({total_threads} threads); \
                         this combination is not supported",
                        conn.arg_name, slave.task, slave.port
                    )));
                }
            }
        }

        Ok(connections)
    }

    /// The `(id_width, threads)` a child port presents to its parent:
    /// the RTL module's AXI ID port width combined with the child's own
    /// nested aggregation (an upper child sharing the mmap internally
    /// presents its aggregated totals; a leaf presents 1 thread).
    fn child_mmap_summary(
        &self,
        child_task_name: &str,
        child_port_name: &str,
        cache: &mut BTreeMap<String, BTreeMap<String, MMapConnection>>,
    ) -> Result<(u32, u32), CodegenError> {
        let child_port = sanitize_array_name(child_port_name);
        let prefix = format!("m_axi_{child_port}");
        let module_id_width = self
            .module_map
            .get(child_task_name)
            .and_then(|module| {
                ["_ARID", "_AWID", "_BID", "_RID"]
                    .iter()
                    .filter_map(|suffix| {
                        module
                            .inner
                            .find_port(&format!("{prefix}{suffix}"))
                            .and_then(tapa_rtl::port::Port::bit_width)
                    })
                    .max()
            })
            .unwrap_or(1);

        let is_upper = self
            .design
            .tasks
            .get(child_task_name)
            .is_some_and(|task| task.level == TaskLevel::Upper);
        let (nested_id_width, threads) = if is_upper {
            if !cache.contains_key(child_task_name) {
                let conns = self.aggregate_mmap_connections_cached(child_task_name, cache)?;
                cache.insert(child_task_name.to_owned(), conns);
            }
            cache[child_task_name]
                .get(child_port_name)
                .map_or((1, 1), |conn| (conn.id_width(), conn.total_threads()))
        } else {
            (1, 1)
        };
        Ok((module_id_width.max(nested_id_width), threads))
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
        || crate::async_mmap::has_direct_m_axi_ports(module, &slave.port)
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

    let tags = crate::async_mmap::active_tags(module, &slave.port);
    if tags.is_empty() {
        return Err(invalid_direct_mmap(
            interface,
            &format!(
                "has async mmap child '{}.{}' with neither FIFO-style async ports nor a compact M-AXI interface",
                slave.task, slave.port
            ),
        ));
    }
    let enabled = crate::async_mmap::enabled_axi_directions(module, &slave.port, &tags);
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

/// Compute parent-facing AXI ID width: 1 + ceil(log2(n)), minimum 1.
fn id_width_for_child_threads(child_id_width: u32, n: u32) -> u32 {
    child_id_width.max(1) + routing_id_bits(n)
}

pub fn routing_id_bits(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    u32::BITS - (n - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_program() -> Design {
        let json = r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "readable_name": "top_task",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "child_a": [{"args": {"data": {"arg": "data", "cat": "istream"}}}]
                    },
                    "fifos": {}
                },
                "child_a": {
                    "readable_name": "child_a",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [{"cat": "istream", "name": "data", "type": "float", "width": 32}],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }"#;
        crate::design_from_fixture_json(serde_json::from_str(json).unwrap())
    }

    fn mmap_geometry_program(
        parent_chan_count: Option<u32>,
        parent_chan_size: Option<u32>,
        child_chan_count: Option<u32>,
        child_chan_size: Option<u32>,
    ) -> Design {
        crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [{
                        "cat": "mmap",
                        "name": "elems",
                        "type": "float*",
                        "width": 32,
                        "chan_count": parent_chan_count,
                        "chan_size": parent_chan_size
                    }],
                    "tasks": {
                        "leaf": [{"args": {
                            "data": {"arg": "elems", "cat": "mmap"}
                        }}]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [{
                        "cat": "mmap",
                        "name": "data",
                        "type": "float*",
                        "width": 32,
                        "chan_count": child_chan_count,
                        "chan_size": child_chan_size
                    }],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }))
    }

    #[test]
    fn attach_module_rejects_unknown_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        let module = VerilogModule::parse("module unknown(); endmodule").unwrap();
        let result = state.attach_module("nonexistent", module);
        assert!(
            matches!(result, Err(CodegenError::TaskNotFound(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn attach_module_rejects_duplicate() {
        let mut state = TopologyWithRtl::new(sample_program());
        let module1 = VerilogModule::parse("module child_a(); endmodule").unwrap();
        let module2 = VerilogModule::parse("module child_a(); endmodule").unwrap();
        state.attach_module("child_a", module1).unwrap();
        let result = state.attach_module("child_a", module2);
        assert!(
            matches!(result, Err(CodegenError::ModuleAlreadyAttached(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn create_fsm_rejects_lower_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        let result = crate::state::views::FsmTable::new(&mut state.fsm_modules)
            .create_fsm_module("child_a", TaskLevel::Lower);
        assert!(
            matches!(result, Err(CodegenError::FsmForLowerTask(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn create_fsm_for_upper_task() {
        let mut state = TopologyWithRtl::new(sample_program());
        crate::state::views::FsmTable::new(&mut state.fsm_modules)
            .create_fsm_module("top_task", TaskLevel::Upper)
            .unwrap();
        assert!(state.fsm_modules.contains_key("top_task"));
    }

    #[test]
    fn id_width_calculation() {
        assert_eq!(id_width_for_child_threads(1, 0), 1);
        assert_eq!(id_width_for_child_threads(1, 1), 1);
        assert_eq!(id_width_for_child_threads(1, 2), 2);
        assert_eq!(id_width_for_child_threads(1, 3), 3);
        assert_eq!(id_width_for_child_threads(1, 4), 3);
        assert_eq!(id_width_for_child_threads(1, 7), 4);
        assert_eq!(id_width_for_child_threads(1, 8), 4);
    }

    #[test]
    fn aggregate_mmap_connections_preserves_child_axi_id_width() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let mut state = TopologyWithRtl::new(program);
        state
            .attach_module(
                "mid",
                VerilogModule::parse(
                    "module mid(\n\
                     output wire [1:0] m_axi_data_ARID,\n\
                     output wire [1:0] m_axi_data_AWID,\n\
                     input wire [1:0] m_axi_data_BID,\n\
                     input wire [1:0] m_axi_data_RID\n\
                     ); endmodule",
                )
                .unwrap(),
            )
            .unwrap();

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            2,
            "parent-facing ID width must be at least as wide as the child AXI port"
        );
    }

    #[test]
    fn aggregate_propagates_nested_slave_threads() {
        // top shares `elems` between a leaf child and an upper child
        // (`mid`) that internally shares its `data` port between two
        // leaves. The parent-facing connection must report the
        // aggregated per-slave thread counts, not a flat 1.
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [{"args": {"d": {"arg": "elems", "cat": "mmap"}}}],
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let mid_conns = state.aggregate_mmap_connections("mid").unwrap();
        let mid_threads: Vec<u32> = mid_conns["data"].slaves.iter().map(|s| s.threads).collect();
        assert_eq!(mid_threads, vec![1, 1]);

        let top_conns = state.aggregate_mmap_connections("top").unwrap();
        let conn = &top_conns["elems"];
        assert_eq!(conn.thread_count(), 2, "two slave ports at top level");
        // Task iteration is alphabetical: `leaf` (1 thread) then `mid`
        // (2 aggregated threads).
        let threads: Vec<u32> = conn.slaves.iter().map(|s| s.threads).collect();
        assert_eq!(threads, vec![1, 2]);
    }

    #[test]
    fn aggregate_rejects_hmap_with_internally_shared_child() {
        // A single hmap child port whose task internally shares the
        // mmap: unsupported, rejected at aggregation time.
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32,
                         "chan_count": 2, "chan_size": 1024}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"d": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);
        let result = state.aggregate_mmap_connections("top");
        assert!(
            matches!(result, Err(CodegenError::InvalidMmapConnection(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn aggregate_rejects_conflicting_channel_shapes() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "chan_leaf": [{"args": {"d": {"arg": "elems", "cat": "mmap"}}}],
                        "plain_leaf": [{"args": {"d": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "chan_leaf": {
                    "readable_name": "chan_leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32,
                         "chan_count": 2, "chan_size": 1024}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "plain_leaf": {
                    "readable_name": "plain_leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);
        let result = state.aggregate_mmap_connections("top");
        assert!(
            matches!(result, Err(CodegenError::InvalidMmapConnection(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn aggregate_rejects_parent_child_data_width_mismatch() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "long*", "width": 64}
                    ],
                    "tasks": {
                        "leaf": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "int*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched AXI widths must be rejected");
        assert!(err.to_string().contains("64 bits"), "got: {err}");
        assert!(err.to_string().contains("32 bits"), "got: {err}");
    }

    #[test]
    fn aggregate_rejects_parent_child_channel_count_mismatch() {
        let state = TopologyWithRtl::new(mmap_geometry_program(
            Some(2),
            Some(1024),
            Some(4),
            Some(1024),
        ));

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched channel counts must be rejected");
        assert!(err.to_string().contains("channel-count mismatch"));
        assert!(err.to_string().contains("top.elems' declares 2"));
        assert!(err.to_string().contains("leaf.data' declares 4"));
    }

    #[test]
    fn aggregate_rejects_parent_child_channel_size_mismatch() {
        let state = TopologyWithRtl::new(mmap_geometry_program(
            Some(2),
            Some(1024),
            Some(2),
            Some(2048),
        ));

        let err = state
            .aggregate_mmap_connections("top")
            .expect_err("mismatched channel sizes must be rejected");
        assert!(err.to_string().contains("channel-size mismatch"));
        assert!(err.to_string().contains("top.elems' declares 1024"));
        assert!(err.to_string().contains("leaf.data' declares 2048"));
    }

    #[test]
    fn aggregate_mmap_connections_preserves_nested_child_crossbar_id_width() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            2,
            "parent-facing ID width should preserve nested child crossbar routing bits"
        );
    }

    #[test]
    fn aggregate_mmap_connections_expands_wide_child_id_for_parent_crossbar() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "readable_name": "top",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}],
                        "leaf": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "mid": {
                    "readable_name": "mid",
                    "level": "upper",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}},
                            {"args": {"mmap": {"arg": "data", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "leaf": {
                    "readable_name": "leaf",
                    "level": "lower",
                    "code": "",
                    "synth": "hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);

        let conns = state.aggregate_mmap_connections("top").unwrap();
        assert_eq!(
            conns["elems"].id_width(),
            3,
            "parent crossbar should append routing bits to the widest child AXI ID"
        );
    }

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
