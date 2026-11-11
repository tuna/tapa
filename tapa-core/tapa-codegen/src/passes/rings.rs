//! Rings: a frame argument of a Vitis top, carried by a ring of frame slots
//! in device memory in place of an AXIS port (see [`tapa_ir::Ring`]). The
//! `frame_ring` mover sits between the argument's channel and an
//! `async_mmap` bridge onto the argument's own M-AXI port; its registers
//! belong to the control slave (see `s_axi`). The channel is the frame
//! channel of a view, or an ordinary FIFO before a leaf's
//! `[[tapa::frame(N)]]` stream port (see [`ring_fifo_width`]). An output
//! ring holds the top's done until everything released into its channel is
//! in memory (its `drained` net joins the children's `is_done` nets).
//!
//! The PL kernel of an AI Engine top (see [`tapa_ir::TopFrames::Rings`])
//! has no channel: each ring moves its frames to or from the argument's
//! PLIO ([`tapa_ir::ring_plio`]), and an output ring's `drained` net is a
//! `ring_count`'s, which ends a run once the frame count the AI Engine
//! sends on [`tapa_ir::ring_count_plio`] is in memory.

use tapa_ir::port::ArgCategory;
use tapa_ir::{Ring, RingRegister};
use tapa_protocol::{
    AXI_ADDR_WIDTH, HANDSHAKE_CLK, HANDSHAKE_IDLE, HANDSHAKE_RST, HANDSHAKE_START, M_AXI_PREFIX,
};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire};

use super::async_mmap::{self, EnabledAxiDirections};
use super::frames::{frame_channel, frame_wire};
use crate::error::CodegenError;
use crate::state::views::{DesignView, ModuleTable};

/// Bits of a ring request's element count (`frame_ring`'s `LEN_WIDTH`).
const LEN_WIDTH: u32 = 32;

/// Elements the ordinary FIFO of a ring holds: the mover moves elements as
/// they come, so it needs no whole frame.
pub(super) const RING_FIFO_DEPTH: u32 = 2;

/// The element bits of the ordinary FIFO that carries the top's ring
/// argument `fifo_name` between its mover and a leaf's `[[tapa::frame(N)]]`
/// stream port, or `None` when the argument has no ring or a view's frame
/// channel carries it.
#[must_use]
pub(super) fn ring_fifo_width(
    graph: &tapa_ir::TaskGraph,
    task_name: &str,
    task: &tapa_ir::Task,
    fifo_name: &str,
) -> Option<u32> {
    let port = task.port(fifo_name)?;
    (task_name == graph.top
        && port.ring.is_some()
        && graph.frame_channel(task, fifo_name).is_none())
    .then_some(port.width)
}

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
        // The argument's frame channel, or for an AI Engine top's PL kernel
        // the argument's PLIO.
        let plio = (!task.fifos.contains_key(&ring.port)).then(|| tapa_ir::ring_plio(&ring.port));
        let width = ring_width(graph, modules, task, &ring, plio.is_some())?;
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
        let plain = plio.is_none() && ring_fifo_width(graph, task_name, task, &ring.port).is_some();
        if let Some(plio) = &plio {
            super::fifos::add_plio_adapter(mm, plio, width, !ring.input);
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
        ports.extend(match &plio {
            // No producer releases frames into a PLIO: the ring's own
            // `drained` goes unused (see `add_ring_count`).
            Some(plio) => channel_ports(
                &ring,
                &sanitize_array_name(plio),
                Expr::lit("1'b0"),
                Expr::lit(""),
            ),
            // An ordinary FIFO releases each element the producer writes.
            None if plain => channel_ports(
                &ring,
                &name,
                Expr::lit(format!("{name}_write & {name}_full_n")),
                Expr::ident(&drained),
            ),
            None => channel_ports(
                &ring,
                &name,
                Expr::ident(frame_wire(
                    &ring.port,
                    ArgCategory::Oframes,
                    "_release_write",
                )),
                Expr::ident(&drained),
            ),
        });
        ports.extend(async_mmap::user_portargs(&bridge, &tags));
        mm.add_instance(
            ModuleInstance::new("frame_ring", format!("{name}_ring"))
                .with_params(ring_params(&ring, width, plain))
                .with_ports(ports),
        );
        if let (Some(_), false) = (&plio, ring.input) {
            add_ring_count(mm, &ring, &drained);
        }
    }
    Ok(())
}

/// The parameters of `ring`'s `frame_ring` mover; `plain` when its channel
/// is an ordinary FIFO (see [`ring_fifo_width`]).
fn ring_params(ring: &Ring, width: u32, plain: bool) -> Vec<ParamArg> {
    let mut params = vec![
        ParamArg::new("DATA_WIDTH", Expr::int(u64::from(width))),
        ParamArg::new("FRAME", Expr::int(u64::from(ring.frame))),
        ParamArg::new("SLOTS", Expr::int(u64::from(ring.slots))),
        ParamArg::new("INPUT", Expr::int(u64::from(ring.input))),
        ParamArg::new("ADDR_WIDTH", Expr::int(u64::from(AXI_ADDR_WIDTH))),
        ParamArg::new("LEN_WIDTH", Expr::int(u64::from(LEN_WIDTH))),
    ];
    if plain && !ring.input {
        params.push(ParamArg::new("RELEASED_ELEMENTS", Expr::int(1)));
    }
    params
}

/// The count PLIO of an AI Engine top's output ring, and the `ring_count`
/// driving the ring's `drained` net from it.
fn add_ring_count(mm: &mut tapa_rtl::mutation::MutableModule, ring: &Ring, drained: &str) {
    let count = tapa_ir::ring_count_plio(&ring.port);
    super::fifos::add_plio_adapter(mm, &count, 32, true);
    let count = sanitize_array_name(&count);
    let [_, kernel_frames, _] = tapa_ir::ring_registers(&ring.port);
    let name = sanitize_array_name(&ring.port);
    mm.add_instance(
        ModuleInstance::new("ring_count", format!("{name}_ring_count")).with_ports(vec![
            PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
            PortArg::new("rst", Expr::ident(HANDSHAKE_RST)),
            PortArg::new(
                "start",
                Expr::lit(format!("{HANDSHAKE_START} & {HANDSHAKE_IDLE}")),
            ),
            PortArg::new("kernel_frames", Expr::ident(kernel_frames.name)),
            PortArg::new("count_dout", Expr::ident(format!("{count}_dout"))),
            PortArg::new("count_empty_n", Expr::ident(format!("{count}_empty_n"))),
            PortArg::new("count_read", Expr::ident(format!("{count}_read"))),
            PortArg::new("ended", Expr::ident(drained)),
        ]),
    );
}

/// The bits of an element of `ring`: its channel's, or for an AI Engine
/// top's PL kernel (`plio`) its argument's.
fn ring_width(
    graph: &tapa_ir::TaskGraph,
    modules: &ModuleTable<'_>,
    task: &tapa_ir::Task,
    ring: &Ring,
    plio: bool,
) -> Result<u32, CodegenError> {
    let unresolved = || CodegenError::FifoWidthUnresolved(ring.port.clone());
    let width = match (plio, task.fifos.get(&ring.port)) {
        (true, _) => task.port(&ring.port).map_or(0, |port| port.width),
        (false, Some(fifo)) => match frame_channel(graph, modules, task, &ring.port, fifo)? {
            Some(channel) => channel.data_width(),
            None => ring_fifo_width(graph, &graph.top, task, &ring.port).ok_or_else(unresolved)?,
        },
        (false, None) => return Err(unresolved()),
    };
    if !(8..=512).contains(&width) || !width.is_power_of_two() {
        return Err(CodegenError::FrameChannel {
            fifo: ring.port.clone(),
            problem: format!(
                "a ring in device memory moves elements of 8 to 512 bits, a \
                 power of two, and this argument's are {width} bits"
            ),
        });
    }
    Ok(width)
}

/// The ring's side of the stream `name` it writes (input) or reads
/// (output): the argument's frame channel, or its PLIO's adapter (see
/// `fifos::add_plio_adapter`). An output ring counts the producer's
/// `released` tokens and drives `drained`.
fn channel_ports(ring: &Ring, name: &str, released: Expr, drained: Expr) -> Vec<PortArg> {
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
        vec![
            PortArg::new("out_din", Expr::lit("")),
            PortArg::new("out_full_n", Expr::lit("1'b0")),
            PortArg::new("out_write", Expr::lit("")),
            PortArg::new("in_dout", wire("_dout")),
            PortArg::new("in_empty_n", wire("_empty_n")),
            PortArg::new("in_read", wire("_read")),
            PortArg::new("released", released),
            PortArg::new("drained", drained),
        ]
    }
}
