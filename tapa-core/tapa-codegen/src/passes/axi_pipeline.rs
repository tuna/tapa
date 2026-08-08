//! Direct M-AXI channel pipeline validation and RTL generation.

use std::collections::BTreeMap;

use tapa_ir::floorplan::Coor;
use tapa_ir::{
    axi_pipeline_instance_name, AxiChannel, AxiEndpoint, FloorplanResult, MemoryBank, RoutedChannel,
};
use tapa_protocol::{
    axi_subport_from_suffix, axi_subport_width, HANDSHAKE_CLK, HANDSHAKE_RST, M_AXI_PREFIX,
    M_AXI_SUFFIXES_BY_CHANNEL, M_AXI_SUFFIXES_COMPACT,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::{sanitize_array_name, sanitize_identifier_name};
use tapa_rtl::mutation::{wide_wire, wire};

use crate::error::CodegenError;
use crate::rtl_state::DirectMmapInterface;
use crate::state::views::ModuleTable;

const OPTIONAL_ADDRESS_OUTPUTS: [(&str, &str); 8] = [
    ("_AWLOCK", "1'b0"),
    ("_AWCACHE", "4'b0011"),
    ("_AWPROT", "3'b000"),
    ("_AWQOS", "4'b0000"),
    ("_ARLOCK", "1'b0"),
    ("_ARCACHE", "4'b0011"),
    ("_ARPROT", "3'b000"),
    ("_ARQOS", "4'b0000"),
];

#[derive(Debug)]
struct PlannedRoute {
    bank: MemoryBank,
    route: Vec<String>,
    reg_regions: Vec<String>,
}

#[derive(Debug)]
struct RoutedEndpoint {
    interface: DirectMmapInterface,
    body_levels: BTreeMap<AxiChannel, u32>,
}

/// Validated direct M-AXI routes for one generated top module.
///
/// An endpoint is absent when it is co-located with its external memory bank.
/// A present endpoint contains exactly one route for every enabled typed AXI
/// channel; read-only and write-only async bridges prune the other group.
#[derive(Debug, Default)]
pub struct DirectAxiPipelinePlan {
    routed: BTreeMap<AxiEndpoint, RoutedEndpoint>,
}

impl DirectAxiPipelinePlan {
    #[allow(
        clippy::too_many_lines,
        reason = "validation keeps the all-or-none five-channel contract in one pass"
    )]
    pub(super) fn from_floorplan(
        interfaces: Vec<DirectMmapInterface>,
        floorplan: &FloorplanResult,
    ) -> Result<Self, CodegenError> {
        let mut expected = BTreeMap::new();
        for interface in interfaces {
            let endpoint = interface.endpoint.clone();
            if expected.insert(endpoint.clone(), interface).is_some() {
                return Err(invalid_floorplan(format!(
                    "direct M-AXI endpoint {} is cataloged more than once",
                    display_endpoint(&endpoint),
                )));
            }
        }

        let mut planned = BTreeMap::<AxiEndpoint, BTreeMap<AxiChannel, PlannedRoute>>::new();
        for route in &floorplan.routes {
            let RoutedChannel::Axi {
                endpoint,
                bank,
                channel,
            } = &route.channel
            else {
                continue;
            };
            let Some(interface) = expected.get(endpoint) else {
                return Err(invalid_floorplan(format!(
                    "AXI route names unknown direct M-AXI endpoint {}",
                    display_endpoint(endpoint),
                )));
            };
            if interface.channel_widths.physical_width(*channel) == 0 {
                return Err(invalid_floorplan(format!(
                    "direct M-AXI endpoint {} has a route for disabled {channel:?} channel",
                    display_endpoint(endpoint),
                )));
            }
            let previous = planned.entry(endpoint.clone()).or_default().insert(
                *channel,
                PlannedRoute {
                    bank: *bank,
                    route: route.route.clone(),
                    reg_regions: route.reg_regions.clone(),
                },
            );
            if previous.is_some() {
                return Err(invalid_floorplan(format!(
                    "direct M-AXI endpoint {} has more than one {:?} route",
                    display_endpoint(endpoint),
                    channel,
                )));
            }
        }

        let mut routed = BTreeMap::new();
        for (endpoint, interface) in expected {
            let child_region = floorplan.regions.get(&endpoint.instance).ok_or_else(|| {
                invalid_floorplan(format!(
                    "direct M-AXI endpoint {} has no instance placement",
                    display_endpoint(&endpoint),
                ))
            })?;
            let child_slot = parse_atomic_region(child_region).ok_or_else(|| {
                invalid_floorplan(format!(
                    "direct M-AXI endpoint {} has non-atomic instance placement '{child_region}'",
                    display_endpoint(&endpoint),
                ))
            })?;

            let Some(channel_routes) = planned.remove(&endpoint) else {
                continue;
            };
            let enabled_channels = interface
                .channel_widths
                .enabled_channels()
                .map(|(channel, _)| channel)
                .collect::<Vec<_>>();
            if channel_routes.len() != enabled_channels.len() {
                let missing = enabled_channels
                    .iter()
                    .filter(|channel| !channel_routes.contains_key(channel))
                    .map(|channel| format!("{channel:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid_floorplan(format!(
                    "direct M-AXI endpoint {} has a partial AXI route set; missing {missing}",
                    display_endpoint(&endpoint),
                )));
            }

            let mut common_bank = None;
            let mut common_bank_slot = None;
            let mut body_levels = BTreeMap::new();
            for channel in enabled_channels {
                let route = &channel_routes[&channel];
                validate_channel_route(
                    &endpoint,
                    channel,
                    route,
                    child_slot,
                    &mut common_bank,
                    &mut common_bank_slot,
                )?;
                let body_level = u32::try_from(route.reg_regions.len()).map_err(|_| {
                    invalid_floorplan(format!(
                        "direct M-AXI endpoint {} {:?} Body level exceeds u32",
                        display_endpoint(&endpoint),
                        channel,
                    ))
                })?;
                for region in &route.reg_regions {
                    if Coor::from_slot_name(region).is_none() {
                        return Err(invalid_floorplan(format!(
                            "direct M-AXI endpoint {} {:?} has invalid Body region '{region}'",
                            display_endpoint(&endpoint),
                            channel,
                        )));
                    }
                }
                body_levels.insert(channel, body_level);
            }
            routed.insert(
                endpoint,
                RoutedEndpoint {
                    interface,
                    body_levels,
                },
            );
        }

        Ok(Self { routed })
    }

    pub(super) fn child_wire_prefix(&self, endpoint: &AxiEndpoint) -> Option<String> {
        self.routed.contains_key(endpoint).then(|| {
            format!(
                "__tapa_axi_{}_child",
                sanitize_identifier_name(&endpoint.top_port),
            )
        })
    }

    pub(super) fn instantiate(
        &self,
        modules: &mut ModuleTable<'_>,
        task_name: &str,
    ) -> Result<(), CodegenError> {
        let Some(module) = modules.get_mut(task_name) else {
            return Ok(());
        };

        for routed in self.routed.values() {
            let interface = &routed.interface;
            let child_prefix = format!(
                "__tapa_axi_{}_child",
                sanitize_identifier_name(&interface.endpoint.top_port),
            );
            let top_prefix = format!(
                "{M_AXI_PREFIX}{}",
                sanitize_array_name(&interface.endpoint.top_port),
            );

            for (channel, _) in interface.channel_widths.enabled_channels() {
                let channel_name = channel_rtl_name(channel);
                for suffix in M_AXI_SUFFIXES_BY_CHANNEL[channel_name]
                    .ports
                    .iter()
                    .filter(|suffix| M_AXI_SUFFIXES_COMPACT.contains(suffix))
                {
                    let width = axi_subport_width(
                        axi_subport_from_suffix(suffix),
                        interface.data_width,
                        interface.addr_width,
                        interface.id_width,
                    );
                    let name = format!("{child_prefix}{suffix}");
                    let signal = if width > 1 {
                        wide_wire(name, &(width - 1).to_string(), "0")
                    } else {
                        wire(name)
                    };
                    module.add_signal(signal)?;
                }
            }

            for (channel, _) in interface.channel_widths.enabled_channels() {
                let body_level = routed.body_levels[&channel];
                module.add_instance(build_channel_instance(
                    interface,
                    channel,
                    body_level,
                    &child_prefix,
                    &top_prefix,
                )?);
            }

            for (suffix, value) in OPTIONAL_ADDRESS_OUTPUTS.into_iter().filter(|(suffix, _)| {
                (suffix.starts_with("_AR") && interface.channel_widths.read_address != 0)
                    || (suffix.starts_with("_AW") && interface.channel_widths.write_address != 0)
            }) {
                module.add_assign(ContinuousAssign::new(
                    Expr::ident(format!("{top_prefix}{suffix}")),
                    Expr::lit(value),
                ));
            }
        }
        Ok(())
    }
}

fn validate_channel_route(
    endpoint: &AxiEndpoint,
    channel: AxiChannel,
    route: &PlannedRoute,
    child_slot: (u32, u32),
    common_bank: &mut Option<MemoryBank>,
    common_bank_slot: &mut Option<(u32, u32)>,
) -> Result<(), CodegenError> {
    if route.route.len() < 2 {
        return Err(invalid_floorplan(format!(
            "direct M-AXI endpoint {} {:?} route must cross at least two slots",
            display_endpoint(endpoint),
            channel,
        )));
    }
    let slots = route
        .route
        .iter()
        .map(|region| {
            Coor::from_slot_name(region)
                .map(|coor| (coor.dl_x, coor.dl_y))
                .ok_or_else(|| {
                    invalid_floorplan(format!(
                        "direct M-AXI endpoint {} {:?} route has invalid slot '{region}'",
                        display_endpoint(endpoint),
                        channel,
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source = slots[0];
    let destination = *slots.last().expect("a two-slot route has a destination");
    let bank_slot = match channel {
        AxiChannel::ReadAddress | AxiChannel::WriteAddress | AxiChannel::WriteData => {
            if source != child_slot {
                return Err(invalid_route_direction(
                    endpoint, channel, "start", source, child_slot,
                ));
            }
            destination
        }
        AxiChannel::ReadData | AxiChannel::WriteResponse => {
            if destination != child_slot {
                return Err(invalid_route_direction(
                    endpoint,
                    channel,
                    "end",
                    destination,
                    child_slot,
                ));
            }
            source
        }
    };
    if bank_slot == child_slot {
        return Err(invalid_floorplan(format!(
            "direct M-AXI endpoint {} {:?} has a route even though both endpoints are in {}",
            display_endpoint(endpoint),
            channel,
            display_slot(child_slot),
        )));
    }

    if let Some(expected) = common_bank {
        if *expected != route.bank {
            return Err(invalid_floorplan(format!(
                "direct M-AXI endpoint {} routes channels to both {expected} and {}",
                display_endpoint(endpoint),
                route.bank,
            )));
        }
    } else {
        *common_bank = Some(route.bank);
    }
    if let Some(expected) = common_bank_slot {
        if *expected != bank_slot {
            return Err(invalid_floorplan(format!(
                "direct M-AXI endpoint {} routes channels to different bank-side slots ({}, {})",
                display_endpoint(endpoint),
                display_slot(*expected),
                display_slot(bank_slot),
            )));
        }
    } else {
        *common_bank_slot = Some(bank_slot);
    }
    Ok(())
}

fn build_channel_instance(
    interface: &DirectMmapInterface,
    channel: AxiChannel,
    body_level: u32,
    child_prefix: &str,
    top_prefix: &str,
) -> Result<ModuleInstance, CodegenError> {
    let channel_name = channel_rtl_name(channel);
    let info = &M_AXI_SUFFIXES_BY_CHANNEL[channel_name];
    let payload_suffixes = info
        .ports
        .iter()
        .copied()
        .filter(|suffix| {
            M_AXI_SUFFIXES_COMPACT.contains(suffix)
                && *suffix != info.valid
                && *suffix != info.ready
        })
        .collect::<Vec<_>>();
    let payload_width = payload_suffixes
        .iter()
        .map(|suffix| {
            axi_subport_width(
                axi_subport_from_suffix(suffix),
                interface.data_width,
                interface.addr_width,
                interface.id_width,
            )
        })
        .sum::<u32>();
    let expected_width = interface
        .channel_widths
        .physical_width(channel)
        .checked_sub(2)
        .ok_or_else(|| {
            invalid_floorplan(format!(
                "direct M-AXI endpoint {} {:?} width does not include VALID and READY",
                display_endpoint(&interface.endpoint),
                channel,
            ))
        })?;
    if payload_width != expected_width || payload_width == 0 {
        return Err(invalid_floorplan(format!(
            "direct M-AXI endpoint {} {:?} payload width is {payload_width}, expected {expected_width}",
            display_endpoint(&interface.endpoint),
            channel,
        )));
    }

    let (source_prefix, destination_prefix) = match channel {
        AxiChannel::ReadAddress | AxiChannel::WriteAddress | AxiChannel::WriteData => {
            (child_prefix, top_prefix)
        }
        AxiChannel::ReadData | AxiChannel::WriteResponse => (top_prefix, child_prefix),
    };
    let concat_payload = |prefix: &str| {
        Expr::concat(
            payload_suffixes
                .iter()
                .map(|suffix| Expr::ident(format!("{prefix}{suffix}")))
                .collect(),
        )
    };

    Ok(ModuleInstance::new(
        "tapa_hs_pipeline",
        axi_pipeline_instance_name(&interface.endpoint, channel),
    )
    .with_params(vec![
        ParamArg::new("DATA_WIDTH", Expr::int(u64::from(payload_width))),
        ParamArg::new("DEPTH", Expr::int(2)),
        ParamArg::new("BODY_LEVEL", Expr::int(u64::from(body_level))),
    ])
    .with_ports(vec![
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("reset", Expr::ident(HANDSHAKE_RST)),
        PortArg::new(
            "if_full_n",
            Expr::ident(format!("{source_prefix}{}", info.ready)),
        ),
        PortArg::new("if_write_ce", Expr::lit("1'b1")),
        PortArg::new(
            "if_write",
            Expr::ident(format!("{source_prefix}{}", info.valid)),
        ),
        PortArg::new("if_din", concat_payload(source_prefix)),
        PortArg::new(
            "if_empty_n",
            Expr::ident(format!("{destination_prefix}{}", info.valid)),
        ),
        PortArg::new("if_read_ce", Expr::lit("1'b1")),
        PortArg::new(
            "if_read",
            Expr::ident(format!("{destination_prefix}{}", info.ready)),
        ),
        PortArg::new("if_dout", concat_payload(destination_prefix)),
    ]))
}

const fn channel_rtl_name(channel: AxiChannel) -> &'static str {
    match channel {
        AxiChannel::ReadAddress => "AR",
        AxiChannel::ReadData => "R",
        AxiChannel::WriteAddress => "AW",
        AxiChannel::WriteData => "W",
        AxiChannel::WriteResponse => "B",
    }
}

fn parse_atomic_region(region: &str) -> Option<(u32, u32)> {
    Coor::from_atomic_region_name(region).map(|coor| (coor.dl_x, coor.dl_y))
}

fn display_endpoint(endpoint: &AxiEndpoint) -> String {
    format!(
        "'{}.{}' (top port '{}')",
        endpoint.instance, endpoint.port, endpoint.top_port,
    )
}

fn display_slot(slot: (u32, u32)) -> String {
    format!("SLOT_X{}Y{}", slot.0, slot.1)
}

fn invalid_route_direction(
    endpoint: &AxiEndpoint,
    channel: AxiChannel,
    endpoint_name: &str,
    actual: (u32, u32),
    expected: (u32, u32),
) -> CodegenError {
    invalid_floorplan(format!(
        "direct M-AXI endpoint {} {:?} route must {endpoint_name} at child slot {}, got {}",
        display_endpoint(endpoint),
        channel,
        display_slot(expected),
        display_slot(actual),
    ))
}

fn invalid_floorplan(detail: impl Into<String>) -> CodegenError {
    CodegenError::InvalidMmapConnection(format!("invalid floorplan: {}", detail.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::MemoryKind;

    const CHILD: (u32, u32) = (1, 0);
    const BANK_SIDE: (u32, u32) = (0, 0);

    fn endpoint() -> AxiEndpoint {
        AxiEndpoint {
            instance: "worker_0".to_owned(),
            port: "mem".to_owned(),
            top_port: "m_axi_mem".to_owned(),
        }
    }

    fn bank(index: u32) -> MemoryBank {
        MemoryBank {
            kind: MemoryKind::Hbm,
            index,
        }
    }

    fn slot((x, y): (u32, u32)) -> String {
        format!("SLOT_X{x}Y{y}")
    }

    /// A route over `slots`, carrying `bank`. `reg_regions` plays no part in
    /// validation, so it stays empty.
    fn route(bank_index: u32, slots: &[(u32, u32)]) -> PlannedRoute {
        PlannedRoute {
            bank: bank(bank_index),
            route: slots.iter().copied().map(slot).collect(),
            reg_regions: Vec::new(),
        }
    }

    /// Validate one channel against fresh accumulators.
    fn check(channel: AxiChannel, route: &PlannedRoute) -> Result<(), CodegenError> {
        validate_channel_route(&endpoint(), channel, route, CHILD, &mut None, &mut None)
    }

    fn message(error: &CodegenError) -> String {
        error.to_string()
    }

    #[test]
    fn forward_channels_start_at_the_child_and_end_at_the_bank() {
        // AR/AW/W carry a request outward, so the child is the source.
        for channel in [
            AxiChannel::ReadAddress,
            AxiChannel::WriteAddress,
            AxiChannel::WriteData,
        ] {
            check(channel, &route(0, &[CHILD, BANK_SIDE]))
                .unwrap_or_else(|e| panic!("{channel:?} should validate: {e}"));
        }
    }

    #[test]
    fn return_channels_start_at_the_bank_and_end_at_the_child() {
        // R/B carry a response back, so the child is the destination.
        for channel in [AxiChannel::ReadData, AxiChannel::WriteResponse] {
            check(channel, &route(0, &[BANK_SIDE, CHILD]))
                .unwrap_or_else(|e| panic!("{channel:?} should validate: {e}"));
        }
    }

    #[test]
    fn a_forward_channel_routed_backwards_is_rejected() {
        // The direction check is the one thing standing between a mirrored
        // floorplan and RTL that wires a request channel the wrong way.
        let error = check(AxiChannel::ReadAddress, &route(0, &[BANK_SIDE, CHILD]))
            .expect_err("a backwards AR route must be rejected");
        assert!(
            message(&error).contains("must start at child slot"),
            "{error}"
        );
    }

    #[test]
    fn a_return_channel_routed_backwards_is_rejected() {
        let error = check(AxiChannel::ReadData, &route(0, &[CHILD, BANK_SIDE]))
            .expect_err("a backwards R route must be rejected");
        assert!(
            message(&error).contains("must end at child slot"),
            "{error}"
        );
    }

    #[test]
    fn a_route_must_cross_at_least_two_slots() {
        let error = check(AxiChannel::ReadAddress, &route(0, &[CHILD]))
            .expect_err("a single-slot route must be rejected");
        assert!(
            message(&error).contains("must cross at least two slots"),
            "{error}"
        );
    }

    #[test]
    fn an_unparsable_slot_name_is_rejected() {
        let bad = PlannedRoute {
            bank: bank(0),
            route: vec![slot(CHILD), "NOT_A_SLOT".to_owned()],
            reg_regions: Vec::new(),
        };
        let error = check(AxiChannel::ReadAddress, &bad)
            .expect_err("an invalid slot name must be rejected");
        assert!(
            message(&error).contains("invalid slot 'NOT_A_SLOT'"),
            "{error}"
        );
    }

    #[test]
    fn a_route_that_never_leaves_the_child_slot_is_rejected() {
        // Co-located endpoints are meant to be absent from the plan
        // entirely, so a route whose two ends agree is a contradiction.
        let error = check(AxiChannel::ReadAddress, &route(0, &[CHILD, CHILD]))
            .expect_err("a route within one slot must be rejected");
        assert!(message(&error).contains("both endpoints are in"), "{error}");
    }

    #[test]
    fn channels_of_one_endpoint_must_agree_on_the_bank() {
        let mut common_bank = None;
        let mut common_slot = None;
        validate_channel_route(
            &endpoint(),
            AxiChannel::ReadAddress,
            &route(0, &[CHILD, BANK_SIDE]),
            CHILD,
            &mut common_bank,
            &mut common_slot,
        )
        .expect("first channel establishes the bank");

        let error = validate_channel_route(
            &endpoint(),
            AxiChannel::ReadData,
            &route(1, &[BANK_SIDE, CHILD]),
            CHILD,
            &mut common_bank,
            &mut common_slot,
        )
        .expect_err("a second bank on one endpoint must be rejected");
        assert!(
            message(&error).contains("routes channels to both"),
            "{error}"
        );
    }

    #[test]
    fn channels_of_one_endpoint_must_agree_on_the_bank_side_slot() {
        let mut common_bank = None;
        let mut common_slot = None;
        validate_channel_route(
            &endpoint(),
            AxiChannel::ReadAddress,
            &route(0, &[CHILD, BANK_SIDE]),
            CHILD,
            &mut common_bank,
            &mut common_slot,
        )
        .expect("first channel establishes the bank-side slot");

        // Same bank, but reached from a different slot.
        let error = validate_channel_route(
            &endpoint(),
            AxiChannel::ReadData,
            &route(0, &[(2, 2), CHILD]),
            CHILD,
            &mut common_bank,
            &mut common_slot,
        )
        .expect_err("two bank-side slots on one endpoint must be rejected");
        assert!(
            message(&error).contains("different bank-side slots"),
            "{error}"
        );
    }

    #[test]
    fn agreeing_channels_accumulate_without_error() {
        let mut common_bank = None;
        let mut common_slot = None;
        for (channel, slots) in [
            (AxiChannel::ReadAddress, [CHILD, BANK_SIDE]),
            (AxiChannel::WriteAddress, [CHILD, BANK_SIDE]),
            (AxiChannel::WriteData, [CHILD, BANK_SIDE]),
            (AxiChannel::ReadData, [BANK_SIDE, CHILD]),
            (AxiChannel::WriteResponse, [BANK_SIDE, CHILD]),
        ] {
            validate_channel_route(
                &endpoint(),
                channel,
                &route(0, &slots),
                CHILD,
                &mut common_bank,
                &mut common_slot,
            )
            .unwrap_or_else(|e| panic!("{channel:?} should validate: {e}"));
        }
        assert_eq!(common_bank, Some(bank(0)));
        assert_eq!(common_slot, Some(BANK_SIDE));
    }
}
