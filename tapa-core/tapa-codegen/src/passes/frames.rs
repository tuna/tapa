//! Frame channels: the storage of a stream with a frame port at either end.
//!
//! A frame port (`tapa::iframes` / `tapa::oframes`) reaches its leaf's RTL
//! as a one-port memory interface plus acquire and release token FIFOs
//! (`tapa_protocol::frame_signals`). Its stream becomes a `frame_channel`
//! instance of `max(2, depth / frame)` banks instead of a FIFO, in the task
//! that declares the stream; an end that is an ordinary stream port connects
//! to the channel's element-stream side. An upper task between the channel
//! and the frame port carries the view's signals through its own port in
//! place of the stream's. A stream the top takes as an argument gets a
//! channel of two banks in the top, whose element side is that argument.
//! A channel whose ends the floorplan puts in different slots is copied
//! (see [`Part`]).

use tapa_ir::port::ArgCategory;
use tapa_ir::{frame_channel_half_instance_names, InterconnectDefinition, Task, TaskGraph};
use tapa_protocol::{
    frame_signal_width, frame_signals, stream_peek_port_name, FrameSignal, PortDir,
    FRAME_BANK_BITS, HANDSHAKE_CLK, HANDSHAKE_RST, ISTREAM_SUFFIXES, OSTREAM_SUFFIXES,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire, MutableModule};

use crate::error::CodegenError;
use crate::state::views::ModuleTable;

/// One end of a frame channel that is a frame port: the child port, its
/// width, and its RTL module when it is attached.
struct FramePort<'a> {
    port: &'a str,
    width: u32,
    module: Option<&'a MutableModule>,
}

impl FramePort<'_> {
    /// The width of the leaf's `{port}{suffix}` signal, if its RTL names it.
    fn rtl_width(&self, suffix: &str) -> Option<u32> {
        let name = format!("{}{suffix}", sanitize_array_name(self.port));
        self.module?.inner.find_port(&name)?.bit_width()
    }
}

/// One frame channel: its shape, and the widths of its memory.
pub(super) struct FrameChannel {
    shape: tapa_ir::FrameChannel,
    data_width: u32,
    addr_width: u32,
}

/// The frame channel `fifo_name` of `task` is, or `None` for a FIFO (see
/// [`TaskGraph::frame_channel`]); the banks must fit the `FRAME_BANK_BITS`
/// token.
pub(super) fn frame_channel(
    graph: &TaskGraph,
    modules: &ModuleTable<'_>,
    task: &Task,
    fifo_name: &str,
    fifo: &InterconnectDefinition,
) -> Result<Option<FrameChannel>, CodegenError> {
    let Some(shape) = graph.frame_channel(task, fifo_name) else {
        return Ok(None);
    };
    let ports: Vec<FramePort<'_>> = [
        (shape.producer_view, fifo.produced_by.as_ref()),
        (shape.consumer_view, fifo.consumed_by.as_ref()),
    ]
    .into_iter()
    .filter(|(view, _)| *view)
    .filter_map(|(_, end)| {
        let end = graph.fifo_endpoint(task, fifo_name, end)?;
        let binding = end.binding?;
        Some(FramePort {
            port: binding.port_name,
            width: binding.port.map_or(0, |port| port.width),
            module: modules.get(&end.reference.0),
        })
    })
    .collect();
    let width = ports.first().map_or(0, |port| port.width);
    let rtl = |suffix: &str| ports.iter().filter_map(|port| port.rtl_width(suffix)).max();
    let data_width = rtl("_buf_d0").or_else(|| rtl("_buf_q0")).unwrap_or(width);
    let addr_width = rtl("_buf_address0")
        .unwrap_or_else(|| frame_signal_width(FrameSignal::Address, width, shape.frame));
    if shape.banks > 1 << FRAME_BANK_BITS {
        return Err(CodegenError::FrameChannel {
            fifo: fifo_name.to_owned(),
            problem: format!(
                "its depth {} holds {} frames of {}, and a frame channel holds \
                 at most {}",
                fifo.depth.unwrap_or(0),
                shape.banks,
                shape.frame,
                1u32 << FRAME_BANK_BITS
            ),
        });
    }
    Ok(Some(FrameChannel {
        shape,
        data_width,
        addr_width,
    }))
}

/// The wire joining one frame-port signal of the channel `fifo`'s producer
/// (`w`) or consumer (`r`) to the channel.
pub(super) fn frame_wire(fifo: &str, cat: ArgCategory, suffix: &str) -> String {
    let side = if cat.is_output_stream() { "w" } else { "r" };
    format!("{}_{side}{suffix}", sanitize_array_name(fifo))
}

/// What one `frame_channel` instance holds of a channel: all of it, or the
/// half at one end when the floorplan puts the ends in different slots.
/// The halves copy frames as an AI Engine tile DMA does: the producer's
/// half streams each released frame out, the stream's pipeline carries the
/// elements across the slots, and the consumer's half fills a frame of its
/// own. An end that is an element stream needs no half: it is the
/// pipeline's own end.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Whole,
    Send,
    Recv,
}

impl FrameChannel {
    /// The instances that hold the channel, split across slots or not.
    pub(super) fn parts(&self, split: bool) -> Vec<Part> {
        if !split {
            return vec![Part::Whole];
        }
        [
            (self.shape.producer_view, Part::Send),
            (self.shape.consumer_view, Part::Recv),
        ]
        .into_iter()
        .filter_map(|(view, part)| view.then_some(part))
        .collect()
    }

    /// The width of the element stream between the halves: an element and
    /// the end-of-transmission bit of the channel's element side.
    pub(super) const fn copy_width(&self) -> u32 {
        self.data_width + 1
    }
}

/// The instance name of `part` of the channel `name`, which of its sides
/// are views (producer, consumer), and the base of its element-side wires.
fn layout(name: &str, channel: &FrameChannel, part: Part) -> (String, [bool; 2], String) {
    let base = sanitize_array_name(name);
    let [send, recv] = frame_channel_half_instance_names(name);
    match part {
        Part::Whole => (
            format!("{base}_fifo"),
            [channel.shape.producer_view, channel.shape.consumer_view],
            base,
        ),
        Part::Send => (send.clone(), [true, false], send),
        Part::Recv => (recv.clone(), [false, true], recv),
    }
}

/// The `frame_channel` instance that holds `holds` of the stream `name`.
pub(super) fn build_frame_channel_instance(
    name: &str,
    channel: &FrameChannel,
    holds: Part,
) -> ModuleInstance {
    let (instance, views, elements) = layout(name, channel, holds);
    let mut ports = vec![
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("reset", Expr::ident(HANDSHAKE_RST)),
    ];
    for (view, cat, stream_suffixes) in [
        (views[0], ArgCategory::Oframes, OSTREAM_SUFFIXES),
        (views[1], ArgCategory::Iframes, ISTREAM_SUFFIXES),
    ] {
        let side = if cat.is_output_stream() { "w" } else { "r" };
        for &(suffix, dir, _) in frame_signals(cat.is_output_stream()) {
            if suffix == "_release_din" {
                continue;
            }
            let port = format!("{side}{suffix}");
            let connection = if view {
                Expr::ident(frame_wire(name, cat, suffix))
            } else if dir == PortDir::Output {
                Expr::lit("'d0") // an input of the channel nothing drives
            } else {
                continue;
            };
            ports.push(PortArg::new(port, connection));
        }
        for suffix in stream_suffixes {
            let port = format!("if{suffix}");
            if !view {
                ports.push(PortArg::new(
                    port,
                    Expr::ident(format!("{elements}{suffix}")),
                ));
            } else if matches!(*suffix, "_din" | "_write" | "_read") {
                ports.push(PortArg::new(port, Expr::lit("'d0")));
            }
        }
    }
    ModuleInstance::new("frame_channel", instance)
        .with_params(vec![
            ParamArg::new("DATA_WIDTH", Expr::int(u64::from(channel.data_width))),
            ParamArg::new("FRAME", Expr::int(u64::from(channel.shape.frame))),
            ParamArg::new("BANKS", Expr::int(u64::from(channel.shape.banks))),
            ParamArg::new("ADDR_WIDTH", Expr::int(u64::from(channel.addr_width))),
            ParamArg::new("BANK_WIDTH", Expr::int(u64::from(FRAME_BANK_BITS))),
            ParamArg::new("W_VIEW", Expr::int(u64::from(views[0]))),
            ParamArg::new("R_VIEW", Expr::int(u64::from(views[1]))),
        ])
        .with_ports(ports)
}

/// Declare the wires between the frame channel `name` and its two ends.
/// Split across slots, its halves join the stream's pipeline, whose wires
/// (`{name}{suffix}`, the ones an element-stream end binds) it declares.
pub(super) fn declare_frame_channel_wires(
    mm: &mut MutableModule,
    name: &str,
    channel: &FrameChannel,
    split: bool,
) {
    let base = sanitize_array_name(name);
    let msb = |width: u32| (width - 1).to_string();
    let elements = |mm: &mut MutableModule, wires: &str, suffixes: &[&str]| {
        for suffix in suffixes {
            let wire_name = format!("{wires}{suffix}");
            let _ = mm.add_signal(if matches!(*suffix, "_din" | "_dout") {
                wide_wire(&wire_name, &channel.data_width.to_string(), "0")
            } else {
                wire(&wire_name)
            });
        }
    };
    for (view, cat, stream_suffixes) in [
        (
            channel.shape.producer_view,
            ArgCategory::Oframes,
            OSTREAM_SUFFIXES,
        ),
        (
            channel.shape.consumer_view,
            ArgCategory::Iframes,
            ISTREAM_SUFFIXES,
        ),
    ] {
        if !view {
            if !split {
                elements(mm, &base, stream_suffixes);
            }
            continue;
        }
        for &(suffix, _, signal) in frame_signals(cat.is_output_stream()) {
            let width = if signal == FrameSignal::Address {
                channel.addr_width
            } else {
                frame_signal_width(signal, channel.data_width, channel.shape.frame)
            };
            let wire_name = frame_wire(name, cat, suffix);
            let _ = mm.add_signal(if width > 1 {
                wide_wire(&wire_name, &msb(width), "0")
            } else {
                wire(&wire_name)
            });
        }
    }
    if !split {
        return;
    }
    elements(mm, &base, OSTREAM_SUFFIXES);
    elements(mm, &base, ISTREAM_SUFFIXES);
    for part in channel.parts(true) {
        let (_, _, half) = layout(name, channel, part);
        if part == Part::Send {
            elements(mm, &half, ISTREAM_SUFFIXES);
            join_streams(mm, &half, &base);
        } else {
            elements(mm, &half, OSTREAM_SUFFIXES);
            join_streams(mm, &base, &half);
        }
    }
}

/// Move elements from the stream read on the wires `from` (`_dout`,
/// `_empty_n`, `_read`) into the stream written on the wires `to` (`_din`,
/// `_full_n`, `_write`): one whenever one is there and the other has room.
pub(super) fn join_streams(mm: &mut MutableModule, from: &str, to: &str) {
    let signal = |base: &str, suffix: &str| Expr::ident(format!("{base}{suffix}"));
    let moves = || Expr::logical_and(signal(from, "_empty_n"), signal(to, "_full_n"));
    mm.add_assign(ContinuousAssign::new(
        signal(to, "_din"),
        signal(from, "_dout"),
    ));
    mm.add_assign(ContinuousAssign::new(signal(to, "_write"), moves()));
    mm.add_assign(ContinuousAssign::new(signal(from, "_read"), moves()));
}

/// Replace the stream ports of the upper task `task_name` that carry a view
/// (see [`TaskGraph::view_frame`]) with that view's frame-port signals,
/// which its tasks bind straight through.
pub(super) fn thread_views(graph: &TaskGraph, modules: &mut ModuleTable<'_>, task_name: &str) {
    let (Some(task), Some(mm)) = (graph.tasks.get(task_name), modules.get_mut(task_name)) else {
        return;
    };
    for port in &task.ports {
        let Some(frame) = graph.view_frame(task_name, &port.name) else {
            continue;
        };
        let (cat, suffixes) = if port.cat.is_input_stream() {
            (ArgCategory::Iframes, ISTREAM_SUFFIXES)
        } else {
            (ArgCategory::Oframes, OSTREAM_SUFFIXES)
        };
        let base = sanitize_array_name(&port.name);
        let mut stale: Vec<String> = suffixes
            .iter()
            .filter_map(|suffix| mm.inner.get_port_of(&port.name, suffix))
            .map(|port| port.name.clone())
            .collect();
        stale.extend(
            ISTREAM_SUFFIXES
                .iter()
                .map(|suffix| stream_peek_port_name(&base, suffix)),
        );
        for name in stale {
            mm.remove_port(&name);
        }
        let mut ports = Vec::new();
        crate::template::add_frames_ports(&mut ports, &port.name, cat, port.width, frame);
        for port in ports {
            let _ = mm.add_port(port);
        }
    }
}
