//! Rings: a frame argument of a Vitis top, carried by a ring of frame slots
//! in device memory in place of an AXIS port (see [`tapa_ir::Ring`]). The
//! `frame_ring` mover sits between the argument's frame channel and an
//! `async_mmap` bridge onto the argument's own M-AXI port; its registers
//! belong to the control slave (see `s_axi`). An output ring holds the
//! top's done until every frame released into its channel is in memory
//! (its `drained` net joins the children's `is_done` nets).

use tapa_ir::port::ArgCategory;
use tapa_ir::{Ring, RingRegister};
use tapa_protocol::{AXI_ADDR_WIDTH, HANDSHAKE_CLK, HANDSHAKE_RST, M_AXI_PREFIX};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire};

use super::async_mmap::{self, EnabledAxiDirections};
use super::frames::{frame_channel, frame_wire};
use crate::error::CodegenError;
use crate::state::views::{DesignView, ModuleTable};

/// Bits of a ring request's element count (`frame_ring`'s `LEN_WIDTH`).
const LEN_WIDTH: u32 = 32;

/// The control registers of the ring of the top's port `ring`: its base
/// address, then [`tapa_ir::ring_registers`].
#[must_use]
pub(super) fn ring_registers(ring: &str) -> Vec<RingRegister> {
    let mut registers = vec![RingRegister {
        name: format!("{}_offset", sanitize_array_name(ring)),
        bits: AXI_ADDR_WIDTH,
        host_writes: true,
    }];
    registers.extend(tapa_ir::ring_registers(ring));
    registers
}

/// The net that holds while every frame released into the output ring of
/// the top's port `ring` is in memory: one of the top's `is_done` nets.
#[must_use]
pub(in crate::passes) fn drained_name(ring: &str) -> String {
    format!("{}_drained", sanitize_array_name(ring))
}

/// Instantiate the rings of the top `task_name`.
pub(super) fn instantiate_rings(
    design: DesignView<'_>,
    modules: &mut ModuleTable<'_>,
    task_name: &str,
) -> Result<(), CodegenError> {
    let graph = design.design();
    if task_name != graph.top {
        return Ok(());
    }
    let task = &graph.tasks[task_name];
    for ring in graph.rings() {
        let fifo = task
            .fifos
            .get(&ring.port)
            .ok_or_else(|| CodegenError::FifoWidthUnresolved(ring.port.clone()))?;
        let width = frame_channel(graph, modules, task, &ring.port, fifo)?
            .ok_or_else(|| CodegenError::FifoWidthUnresolved(ring.port.clone()))?
            .data_width();
        if !(8..=512).contains(&width) || !width.is_power_of_two() {
            return Err(CodegenError::FrameChannel {
                fifo: ring.port.clone(),
                problem: format!(
                    "a ring in device memory moves elements of 8 to 512 bits, a \
                     power of two, and this argument's are {width} bits"
                ),
            });
        }
        let Some(mm) = modules.get_mut(task_name) else {
            continue;
        };
        let name = sanitize_array_name(&ring.port);
        super::m_axi::add_m_axi_ports(mm, &ring.port, width, AXI_ADDR_WIDTH);
        let bridge = format!("{name}_ring");
        let tags = async_mmap::one_way_tags(ring.input);
        let request_width = AXI_ADDR_WIDTH + LEN_WIDTH;
        async_mmap::add_bridge_signals(mm, &bridge, &tags, width, request_width);
        let m_axi = format!("{M_AXI_PREFIX}{name}");
        mm.add_instance(async_mmap::build_bridge_instance(
            &bridge,
            &format!("{bridge}_mmap"),
            &m_axi,
            &m_axi,
            &tags,
            EnabledAxiDirections {
                read: ring.input,
                write: !ring.input,
            },
            width,
            request_width,
            true,
            Expr::ident(HANDSHAKE_RST),
        ));
        let drained = drained_name(&ring.port);
        if !ring.input {
            let _ = mm.add_signal(wire(&drained));
        }
        let mut ports = vec![
            PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
            PortArg::new("rst", Expr::ident(HANDSHAKE_RST)),
        ];
        for (register, port) in ring_registers(&ring.port).into_iter().zip([
            "base",
            "host_frames",
            "kernel_frames",
            "shape",
        ]) {
            let msb = (register.bits - 1).to_string();
            let _ = mm.add_signal(wide_wire(&register.name, &msb, "0"));
            ports.push(PortArg::new(port, Expr::ident(register.name)));
        }
        ports.extend(channel_ports(&ring, &name, &drained));
        ports.extend(async_mmap::user_portargs(&bridge, &tags));
        mm.add_instance(
            ModuleInstance::new("frame_ring", format!("{name}_ring"))
                .with_params(vec![
                    ParamArg::new("DATA_WIDTH", Expr::int(u64::from(width))),
                    ParamArg::new("FRAME", Expr::int(u64::from(ring.frame))),
                    ParamArg::new("SLOTS", Expr::int(u64::from(ring.slots))),
                    ParamArg::new("INPUT", Expr::int(u64::from(ring.input))),
                    ParamArg::new("ADDR_WIDTH", Expr::int(u64::from(AXI_ADDR_WIDTH))),
                    ParamArg::new("LEN_WIDTH", Expr::int(u64::from(LEN_WIDTH))),
                ])
                .with_ports(ports),
        );
    }
    Ok(())
}

/// The ring's side of the argument's frame channel: the channel's element
/// stream, which the ring writes (input) or reads (output), and for an
/// output the producer's release tokens it counts.
fn channel_ports(ring: &Ring, name: &str, drained: &str) -> Vec<PortArg> {
    let wire = |suffix: &str| Expr::ident(format!("{name}{suffix}"));
    if ring.input {
        vec![
            PortArg::new("out_din", wire("_din")),
            PortArg::new("out_full_n", wire("_full_n")),
            PortArg::new("out_write", wire("_write")),
            PortArg::new("in_dout", Expr::lit("'d0")),
            PortArg::new("in_empty_n", Expr::lit("1'b0")),
            PortArg::new("in_read", Expr::lit("")),
            PortArg::new("released", Expr::lit("1'b0")),
            PortArg::new("drained", Expr::lit("")),
        ]
    } else {
        let released = frame_wire(&ring.port, ArgCategory::Oframes, "_release_write");
        vec![
            PortArg::new("out_din", Expr::lit("")),
            PortArg::new("out_full_n", Expr::lit("1'b0")),
            PortArg::new("out_write", Expr::lit("")),
            PortArg::new("in_dout", wire("_dout")),
            PortArg::new("in_empty_n", wire("_empty_n")),
            PortArg::new("in_read", wire("_read")),
            PortArg::new("released", Expr::ident(released)),
            PortArg::new("drained", Expr::ident(drained)),
        ]
    }
}
