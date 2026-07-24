//! Validation and RTL wiring for floorplanned distributed control.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::{
    control_pipeline_instance_name, global_controller_instance_name,
    local_controller_instance_name, ControlChannel, PipelineScheme, RoutedChannel,
};
use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST_N,
    HANDSHAKE_START,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::{sanitize_array_name, sanitize_identifier_name};
use tapa_rtl::mutation::{wide_wire, wire};

use crate::error::CodegenError;
use crate::instance_signals::InstanceSignals;
use crate::rtl_state::{MMapConnection, TopologyWithRtl};

const GLOBAL_START: &str = "__tapa_control_start";
const GLOBAL_RELEASE: &str = "__tapa_control_release";
const CHILDREN_DONE: &str = "__tapa_control_children_done";
const CHILDREN_CLEAR: &str = "__tapa_control_children_clear";
pub const FABRIC_RESET_N: &str = "__tapa_control_fabric_reset_n";

/// `max_fanout` applied to the active-high reset net (`ap_rst`) in
/// floorplanned builds so Vivado replicates the driver and keeps reset
/// distribution local to each SLR. The fabric reset otherwise fans out to
/// every FIFO/AXI endpoint across all SLRs.
pub const FABRIC_RESET_MAX_FANOUT: u32 = 256;

/// The active-high reset signal declaration, with a `max_fanout` attribute
/// when distributed control is active (the floorplanned fabric reset) and a
/// plain wire otherwise, preserving legacy RTL byte-for-byte.
#[must_use]
pub fn fabric_reset_signal(distributed_control: bool) -> tapa_rtl::signal::Signal {
    use tapa_protocol::HANDSHAKE_RST;
    use tapa_rtl::mutation::{wire, wire_with_attribute};
    if distributed_control {
        wire_with_attribute(
            HANDSHAKE_RST,
            format!("max_fanout = {FABRIC_RESET_MAX_FANOUT}"),
        )
    } else {
        wire(HANDSHAKE_RST)
    }
}

struct ChildEntry {
    instance: String,
    definition: String,
    is_autorun: bool,
    args: BTreeMap<String, tapa_ir::Arg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedForwardRoute {
    path: Vec<String>,
    scheme: PipelineScheme,
    reg_regions: Vec<String>,
    body_level: u32,
    latency: u32,
}

#[derive(Debug)]
struct PayloadField {
    output: String,
    source: Expr,
    width: u32,
    lsb: u32,
}

#[derive(Debug)]
struct ChildControlPlan {
    is_autorun: bool,
    launch_width: u32,
    payload: Vec<PayloadField>,
    launch: Option<FeedForwardRoute>,
    reset: Option<FeedForwardRoute>,
    completion: Option<FeedForwardRoute>,
}

/// A fully validated distributed-control projection for one flattened top.
///
/// Construction is read-only and happens before codegen clears the attached
/// HLS body. Once this value exists, every controller placement and every
/// required typed route is known to be complete and directionally consistent.
#[derive(Debug)]
pub struct DistributedControlPlan {
    children: BTreeMap<String, ChildControlPlan>,
    flush_cycles: u32,
}

impl DistributedControlPlan {
    #[allow(
        clippy::too_many_lines,
        reason = "the exact route-set validation is clearest as one transaction"
    )]
    pub(super) fn from_floorplan(
        state: &TopologyWithRtl,
        task_name: &str,
        mmap_connections: &BTreeMap<String, MMapConnection>,
    ) -> Result<Option<Self>, CodegenError> {
        let Some(floorplan) = state.floorplan.as_ref() else {
            return Ok(None);
        };
        let has_marker = floorplan
            .regions
            .contains_key(global_controller_instance_name());
        let has_routes = floorplan
            .routes
            .iter()
            .any(|route| matches!(route.channel, RoutedChannel::Control { .. }));
        if !has_marker {
            if has_routes {
                return Err(invalid_floorplan(
                    "control routes are present without a global controller placement",
                ));
            }
            return Ok(None);
        }
        if task_name != state.design.top || !state.supports_distributed_control() {
            return Err(invalid_floorplan(format!(
                "global controller placement cannot be realized for task '{task_name}'",
            )));
        }

        let task = state
            .design
            .tasks
            .get(task_name)
            .ok_or_else(|| CodegenError::TaskNotFound(task_name.to_owned()))?;
        let global_region = floorplan
            .regions
            .get(global_controller_instance_name())
            .expect("marker was checked");
        match (
            state.top_instantiates_control_s_axi(),
            floorplan.regions.get("control_s_axi_U"),
        ) {
            (true, None) => {
                return Err(invalid_floorplan(
                    "top control AXI-Lite is missing placement 'control_s_axi_U'",
                ));
            }
            (true, Some(control_region)) if control_region != global_region => {
                return Err(invalid_floorplan(format!(
                    "control_s_axi_U is placed in '{control_region}', expected global controller region '{global_region}'",
                )));
            }
            (false, Some(_)) => {
                return Err(invalid_floorplan(
                    "unexpected control_s_axi_U placement without a generated top control AXI-Lite block",
                ));
            }
            (true, Some(_)) | (false, None) => {}
        }
        let global_slot = parse_atomic_region(global_region).ok_or_else(|| {
            invalid_floorplan(format!(
                "global controller has non-atomic placement '{global_region}'",
            ))
        })?;

        let mut child_entries = Vec::<ChildEntry>::new();
        let mut known_instances = BTreeSet::new();
        for (child_name, instances) in &task.tasks {
            for (index, instance) in instances.iter().enumerate() {
                let logical_name = instance.canonical_name(child_name, index).into_owned();
                if !known_instances.insert(logical_name.clone()) {
                    return Err(invalid_floorplan(format!(
                        "flattened child instance '{logical_name}' is not unique",
                    )));
                }
                child_entries.push(ChildEntry {
                    instance: logical_name,
                    definition: child_name.clone(),
                    is_autorun: instance.step < 0,
                    args: instance.args.clone(),
                });
            }
        }

        let mut planned_routes = BTreeMap::<(String, ControlChannel), FeedForwardRoute>::new();
        let mut max_data_latency = 0_u32;
        for route in &floorplan.routes {
            let body_level = u32::try_from(route.reg_regions.len())
                .map_err(|_| invalid_floorplan("pipeline Body level exceeds u32"))?;
            let latency = body_level
                .checked_add(2)
                .ok_or_else(|| invalid_floorplan("pipeline latency exceeds u32"))?;
            let RoutedChannel::Control { instance, channel } = &route.channel else {
                max_data_latency = max_data_latency.max(latency);
                continue;
            };
            if !known_instances.contains(instance) {
                return Err(invalid_floorplan(format!(
                    "control route names unknown child instance '{instance}'",
                )));
            }
            let planned = FeedForwardRoute {
                path: route.route.clone(),
                scheme: route.scheme,
                reg_regions: route.reg_regions.clone(),
                body_level,
                latency,
            };
            if planned_routes
                .insert((instance.clone(), *channel), planned)
                .is_some()
            {
                return Err(invalid_floorplan(format!(
                    "child '{instance}' has more than one {channel:?} route",
                )));
            }
        }

        let mut children = BTreeMap::new();
        let mut flush_cycles = 0;
        let mut max_reset_latency = 0;
        for ChildEntry {
            instance,
            definition,
            is_autorun,
            args,
        } in child_entries
        {
            let child_region = floorplan.regions.get(&instance).ok_or_else(|| {
                invalid_floorplan(format!("child '{instance}' has no instance placement"))
            })?;
            let child_slot = parse_atomic_region(child_region).ok_or_else(|| {
                invalid_floorplan(format!(
                    "child '{instance}' has non-atomic placement '{child_region}'",
                ))
            })?;
            let local_name = local_controller_instance_name(&instance);
            let local_region = floorplan.regions.get(&local_name).ok_or_else(|| {
                invalid_floorplan(format!(
                    "child '{instance}' has no local controller placement '{local_name}'",
                ))
            })?;
            if local_region != child_region {
                return Err(invalid_floorplan(format!(
                    "local controller '{local_name}' is placed in '{local_region}', expected '{child_region}'",
                )));
            }

            let crossing = child_slot != global_slot;
            let launch = take_expected_route(
                &mut planned_routes,
                &instance,
                ControlChannel::Launch,
                crossing,
                global_slot,
                child_slot,
            )?;
            let reset = take_expected_route(
                &mut planned_routes,
                &instance,
                ControlChannel::Reset,
                crossing,
                global_slot,
                child_slot,
            )?;
            if is_autorun
                && planned_routes.contains_key(&(instance.clone(), ControlChannel::Completion))
            {
                return Err(invalid_floorplan(format!(
                    "autorun child '{instance}' must not have a Completion route",
                )));
            }
            let completion = if is_autorun {
                None
            } else {
                take_expected_route(
                    &mut planned_routes,
                    &instance,
                    ControlChannel::Completion,
                    crossing,
                    child_slot,
                    global_slot,
                )?
            };
            if let (Some(launch), Some(reset)) = (&launch, &reset) {
                if launch != reset {
                    return Err(invalid_floorplan(format!(
                        "child '{instance}' Launch and Reset routes must be identical",
                    )));
                }
            }

            let (payload, launch_width) = build_payload_fields(
                state,
                &definition,
                &instance,
                is_autorun,
                &args,
                mmap_connections,
            )?;
            let reset_latency = reset.as_ref().map_or(0, |route| route.latency);
            let completion_latency = completion.as_ref().map_or(0, |route| route.latency);
            max_reset_latency = max_reset_latency.max(reset_latency);
            flush_cycles =
                flush_cycles.max(reset_latency.checked_add(completion_latency).ok_or_else(
                    || {
                        invalid_floorplan(format!(
                            "child '{instance}' reset flush latency exceeds u32",
                        ))
                    },
                )?);
            children.insert(
                instance,
                ChildControlPlan {
                    is_autorun,
                    launch_width,
                    payload,
                    launch,
                    reset,
                    completion,
                },
            );
        }

        if let Some(((instance, channel), _)) = planned_routes.into_iter().next() {
            return Err(invalid_floorplan(format!(
                "child '{instance}' has unexpected {channel:?} route",
            )));
        }
        flush_cycles = flush_cycles.max(
            max_reset_latency
                .checked_add(max_data_latency)
                .ok_or_else(|| invalid_floorplan("data-fabric reset latency exceeds u32"))?,
        );
        Ok(Some(Self {
            children,
            flush_cycles,
        }))
    }

    pub(super) fn child_reset_name(instance: &str) -> String {
        format!("{}__reset_n", control_wire_prefix(instance))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "child control wiring is kept together so bundle slices remain auditable"
    )]
    pub(super) fn instantiate_child(
        &self,
        state: &mut TopologyWithRtl,
        task_name: &str,
        logical_instance: &str,
        signals: &InstanceSignals,
    ) -> Result<(), CodegenError> {
        let child = self.children.get(logical_instance).ok_or_else(|| {
            invalid_floorplan(format!(
                "generated child '{logical_instance}' has no control plan",
            ))
        })?;
        let Some(module) = state.module_map.get_mut(task_name) else {
            return Ok(());
        };
        let prefix = control_wire_prefix(logical_instance);
        let launch_input = format!("{prefix}__launch_input");
        let launch_output = format!("{prefix}__launch_output");
        let reset_n = Self::child_reset_name(logical_instance);
        let local_completion = format!("{prefix}__local_completion");

        add_width_wire(module, &launch_input, child.launch_width)?;
        add_width_wire(module, &launch_output, child.launch_width)?;
        module.add_signal(wire(&reset_n))?;
        module.add_signal(wire(&local_completion))?;

        let mut launch_fields = child
            .payload
            .iter()
            .rev()
            .map(|field| field.source.clone())
            .collect::<Vec<_>>();
        if !child.is_autorun {
            launch_fields.push(Expr::ident(GLOBAL_RELEASE));
        }
        launch_fields.push(Expr::ident(GLOBAL_START));
        module.add_assign(ContinuousAssign::new(
            Expr::ident(&launch_input),
            concat_or_scalar(launch_fields),
        ));

        instantiate_or_connect_pipeline(
            module,
            logical_instance,
            ControlChannel::Launch,
            child.launch.as_ref(),
            child.launch_width,
            Expr::ident(&launch_input),
            Expr::ident(&launch_output),
        );
        instantiate_or_connect_pipeline(
            module,
            logical_instance,
            ControlChannel::Reset,
            child.reset.as_ref(),
            1,
            Expr::ident(HANDSHAKE_RST_N),
            Expr::ident(&reset_n),
        );

        for field in &child.payload {
            add_width_wire(module, &field.output, field.width)?;
            module.add_assign(ContinuousAssign::new(
                Expr::ident(&field.output),
                select_bits(&launch_output, field.lsb, field.width),
            ));
        }

        let launch_start = if child.launch_width == 1 {
            Expr::ident(&launch_output)
        } else {
            Expr::index(Expr::ident(&launch_output), Expr::int(0))
        };
        let launch_release = if child.is_autorun {
            Expr::lit("1'b0")
        } else {
            Expr::index(Expr::ident(&launch_output), Expr::int(1))
        };
        let child_done = if child.is_autorun {
            Expr::lit("1'b0")
        } else {
            Expr::ident(signals.done_name())
        };
        let child_ready = if child.is_autorun {
            Expr::lit("1'b0")
        } else {
            Expr::ident(signals.ready_name())
        };
        let child_idle = if child.is_autorun {
            Expr::lit("1'b0")
        } else {
            Expr::ident(signals.idle_name())
        };
        module.add_instance(
            ModuleInstance::new(
                "tapa_local_controller",
                local_controller_instance_name(logical_instance),
            )
            .with_params(vec![ParamArg::new(
                "AUTORUN",
                Expr::int(u64::from(child.is_autorun)),
            )])
            .with_ports(vec![
                PortArg::new("ap_clk", Expr::ident(HANDSHAKE_CLK)),
                PortArg::new("reset_n", Expr::ident(&reset_n)),
                PortArg::new("launch_start", launch_start),
                PortArg::new("launch_release", launch_release),
                PortArg::new("child_done", child_done),
                PortArg::new("child_ready", child_ready),
                PortArg::new("child_idle", child_idle),
                PortArg::new("child_start", Expr::ident(signals.start_name())),
                PortArg::new("completion", Expr::ident(&local_completion)),
            ]),
        );

        if !child.is_autorun {
            instantiate_or_connect_pipeline(
                module,
                logical_instance,
                ControlChannel::Completion,
                child.completion.as_ref(),
                1,
                Expr::ident(&local_completion),
                Expr::ident(signals.is_done_name()),
            );
        }
        Ok(())
    }

    pub(super) fn instantiate_global(
        &self,
        state: &mut TopologyWithRtl,
        task_name: &str,
        completion_signals: &[String],
    ) -> Result<(), CodegenError> {
        let Some(module) = state.module_map.get_mut(task_name) else {
            return Ok(());
        };
        module.add_signal(wire(GLOBAL_START))?;
        module.add_signal(wire(GLOBAL_RELEASE))?;
        module.add_signal(wire(CHILDREN_DONE))?;
        module.add_signal(wire(CHILDREN_CLEAR))?;
        module.add_signal(wire(FABRIC_RESET_N))?;

        let done = logical_reduction(
            completion_signals
                .iter()
                .map(|signal| Expr::ident(signal.clone())),
            "1'b1",
        );
        let clear = logical_reduction(
            completion_signals
                .iter()
                .map(|signal| Expr::logical_not(Expr::ident(signal.clone()))),
            "1'b1",
        );
        module.add_assign(ContinuousAssign::new(Expr::ident(CHILDREN_DONE), done));
        module.add_assign(ContinuousAssign::new(Expr::ident(CHILDREN_CLEAR), clear));
        module.add_instance(
            ModuleInstance::new("tapa_global_controller", global_controller_instance_name())
                .with_params(vec![ParamArg::new(
                    "FLUSH_CYCLES",
                    Expr::int(u64::from(self.flush_cycles)),
                )])
                .with_ports(vec![
                    PortArg::new("ap_clk", Expr::ident(HANDSHAKE_CLK)),
                    PortArg::new("ap_rst_n", Expr::ident(HANDSHAKE_RST_N)),
                    PortArg::new("ap_start", Expr::ident(HANDSHAKE_START)),
                    PortArg::new("children_done", Expr::ident(CHILDREN_DONE)),
                    PortArg::new("children_clear", Expr::ident(CHILDREN_CLEAR)),
                    PortArg::new("launch_start", Expr::ident(GLOBAL_START)),
                    PortArg::new("launch_release", Expr::ident(GLOBAL_RELEASE)),
                    PortArg::new("fabric_reset_n", Expr::ident(FABRIC_RESET_N)),
                    PortArg::new("ap_done", Expr::ident(HANDSHAKE_DONE)),
                    PortArg::new("ap_ready", Expr::ident(HANDSHAKE_READY)),
                    PortArg::new("ap_idle", Expr::ident(HANDSHAKE_IDLE)),
                ]),
        );
        Ok(())
    }
}

fn take_expected_route(
    routes: &mut BTreeMap<(String, ControlChannel), FeedForwardRoute>,
    instance: &str,
    channel: ControlChannel,
    expected: bool,
    source: (u32, u32),
    destination: (u32, u32),
) -> Result<Option<FeedForwardRoute>, CodegenError> {
    let key = (instance.to_owned(), channel);
    let route = routes.remove(&key);
    match (expected, route) {
        (false, None) => Ok(None),
        (false, Some(_)) => Err(invalid_floorplan(format!(
            "child '{instance}' has a {channel:?} route despite co-located endpoints",
        ))),
        (true, None) => Err(invalid_floorplan(format!(
            "cross-slot child '{instance}' is missing its {channel:?} route",
        ))),
        (true, Some(route)) => {
            validate_route(instance, channel, &route, source, destination)?;
            Ok(Some(route))
        }
    }
}

fn validate_route(
    instance: &str,
    channel: ControlChannel,
    route: &FeedForwardRoute,
    source: (u32, u32),
    destination: (u32, u32),
) -> Result<(), CodegenError> {
    if route.path.len() < 2 {
        return Err(invalid_floorplan(format!(
            "child '{instance}' {channel:?} route must cross at least two slots",
        )));
    }
    let slots = route
        .path
        .iter()
        .map(|region| {
            parse_slot(region).ok_or_else(|| {
                invalid_floorplan(format!(
                    "child '{instance}' {channel:?} route has invalid slot '{region}'",
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if slots.first() != Some(&source) || slots.last() != Some(&destination) {
        return Err(invalid_floorplan(format!(
            "child '{instance}' {channel:?} route has inconsistent direction",
        )));
    }
    for region in &route.reg_regions {
        if parse_slot(region).is_none() {
            return Err(invalid_floorplan(format!(
                "child '{instance}' {channel:?} has invalid Body region '{region}'",
            )));
        }
        if !route.path.contains(region) {
            return Err(invalid_floorplan(format!(
                "child '{instance}' {channel:?} Body region '{region}' is not on its route",
            )));
        }
    }
    Ok(())
}

fn build_payload_fields(
    state: &TopologyWithRtl,
    child_name: &str,
    instance: &str,
    is_autorun: bool,
    args: &BTreeMap<String, tapa_ir::Arg>,
    mmap_connections: &BTreeMap<String, MMapConnection>,
) -> Result<(Vec<PayloadField>, u32), CodegenError> {
    let control_width = if is_autorun { 1 } else { 2 };
    let mut next_lsb = control_width;
    let mut fields = Vec::new();
    let rtl_instance = sanitize_identifier_name(instance);
    for (port_name, arg) in args {
        let (output, source, width) = if arg.cat.is_scalar() {
            (
                format!("{rtl_instance}__{port_name}"),
                Expr::ident(sanitize_array_name(&arg.arg)),
                resolve_scalar_width(state, child_name, port_name)?,
            )
        } else if arg.cat.is_direct_mmap() {
            let parent_name = sanitize_array_name(&arg.arg);
            let source = mmap_connections.get(&arg.arg).map_or_else(
                || Expr::ident(format!("{parent_name}_offset")),
                |connection| {
                    if connection.chan_count.is_some() {
                        Expr::lit("64'd0")
                    } else {
                        Expr::ident(format!("{parent_name}_offset"))
                    }
                },
            );
            (format!("{rtl_instance}__{port_name}_offset"), source, 64)
        } else {
            continue;
        };
        if width == 0 {
            return Err(invalid_floorplan(format!(
                "child '{instance}' argument '{port_name}' has zero width",
            )));
        }
        fields.push(PayloadField {
            output,
            source,
            width,
            lsb: next_lsb,
        });
        next_lsb = next_lsb.checked_add(width).ok_or_else(|| {
            invalid_floorplan(format!("child '{instance}' Launch width exceeds u32"))
        })?;
    }
    Ok((fields, next_lsb))
}

fn resolve_scalar_width(
    state: &TopologyWithRtl,
    child_name: &str,
    port_name: &str,
) -> Result<u32, CodegenError> {
    let rtl_width = state
        .module_map
        .get(child_name)
        .and_then(|module| module.inner.find_port(port_name))
        .and_then(tapa_rtl::port::Port::bit_width);
    let topology_width = state
        .design
        .tasks
        .get(child_name)
        .and_then(|task| task.ports.iter().find(|port| port.name == port_name))
        .map(|port| port.width);
    let topology_width = topology_width.filter(|width| *width > 0).ok_or_else(|| {
        invalid_floorplan(format!(
            "cannot resolve topology scalar width for '{child_name}.{port_name}'",
        ))
    })?;
    if let Some(rtl_width) = rtl_width {
        if rtl_width != topology_width {
            return Err(invalid_floorplan(format!(
                "scalar width mismatch for '{child_name}.{port_name}': topology is {topology_width} bits but RTL is {rtl_width} bits",
            )));
        }
    }
    Ok(topology_width)
}

fn instantiate_or_connect_pipeline(
    module: &mut tapa_rtl::mutation::MutableModule,
    instance: &str,
    channel: ControlChannel,
    route: Option<&FeedForwardRoute>,
    width: u32,
    input: Expr,
    output: Expr,
) {
    if let Some(route) = route {
        module.add_instance(
            ModuleInstance::new(
                "tapa_control_pipeline",
                control_pipeline_instance_name(instance, channel),
            )
            .with_params(vec![
                ParamArg::new("WIDTH", Expr::int(u64::from(width))),
                ParamArg::new("BODY_LEVEL", Expr::int(u64::from(route.body_level))),
            ])
            .with_ports(vec![
                PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
                PortArg::new("in_data", input),
                PortArg::new("out_data", output),
            ]),
        );
    } else {
        module.add_assign(ContinuousAssign::new(output, input));
    }
}

fn add_width_wire(
    module: &mut tapa_rtl::mutation::MutableModule,
    name: &str,
    width: u32,
) -> Result<(), CodegenError> {
    if width == 1 {
        module.add_signal(wire(name))?;
    } else {
        module.add_signal(wide_wire(name, &(width - 1).to_string(), "0"))?;
    }
    Ok(())
}

fn select_bits(signal: &str, lsb: u32, width: u32) -> Expr {
    if width == 1 {
        Expr::index(Expr::ident(signal), Expr::int(u64::from(lsb)))
    } else {
        Expr::range(
            Expr::ident(signal),
            Expr::int(u64::from(lsb + width - 1)),
            Expr::int(u64::from(lsb)),
        )
    }
}

fn concat_or_scalar(mut fields: Vec<Expr>) -> Expr {
    if fields.len() == 1 {
        fields.pop().expect("one field")
    } else {
        Expr::concat(fields)
    }
}

fn logical_reduction(expressions: impl Iterator<Item = Expr>, identity: &str) -> Expr {
    expressions
        .reduce(Expr::logical_and)
        .unwrap_or_else(|| Expr::lit(identity))
}

fn control_wire_prefix(instance: &str) -> String {
    format!("__tapa_control_{}", sanitize_identifier_name(instance))
}

fn parse_atomic_region(region: &str) -> Option<(u32, u32)> {
    let Some((start, end)) = region.split_once("_TO_") else {
        return parse_slot(region);
    };
    let start = parse_slot(start)?;
    (parse_slot(end)? == start).then_some(start)
}

fn parse_slot(slot: &str) -> Option<(u32, u32)> {
    let coordinates = slot.strip_prefix("SLOT_X")?;
    let (x, y) = coordinates.split_once('Y')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn invalid_floorplan(detail: impl Into<String>) -> CodegenError {
    CodegenError::InvalidFloorplan(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_bit_selection_uses_an_index() {
        assert_eq!(select_bits("bundle", 3, 1).to_string(), "bundle[3]");
    }

    #[test]
    fn multi_bit_selection_uses_the_exact_range() {
        assert_eq!(select_bits("bundle", 2, 64).to_string(), "bundle[65:2]");
    }

    #[test]
    fn atomic_region_rejects_a_range() {
        assert_eq!(parse_atomic_region("SLOT_X1Y2"), Some((1, 2)));
        assert_eq!(parse_atomic_region("SLOT_X1Y2_TO_SLOT_X1Y2"), Some((1, 2)));
        assert_eq!(parse_atomic_region("SLOT_X1Y2_TO_SLOT_X2Y2"), None);
    }
}
