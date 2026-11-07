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

use tapa_ir::port::ArgCategory;
use tapa_ir::{FifoEndpoint, InterconnectDefinition, PortBinding, Task, TaskGraph};
use tapa_protocol::{
    frame_signal_width, frame_signals, stream_peek_port_name, FrameSignal, PortDir,
    FRAME_BANK_BITS, HANDSHAKE_CLK, HANDSHAKE_RST, ISTREAM_SUFFIXES, OSTREAM_SUFFIXES,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire, MutableModule};

use crate::error::CodegenError;
use crate::state::views::ModuleTable;

/// One end of a frame channel that is a frame port: the child port, and its
/// RTL module when it is attached.
struct FramePort<'a> {
    port: &'a str,
    module: Option<&'a MutableModule>,
}

impl FramePort<'_> {
    /// The width of the leaf's `{port}{suffix}` signal, if its RTL names it.
    fn rtl_width(&self, suffix: &str) -> Option<u32> {
        let name = format!("{}{suffix}", sanitize_array_name(self.port));
        self.module?.inner.find_port(&name)?.bit_width()
    }
}

/// The shape of one frame channel.
pub(super) struct FrameChannel {
    frame: u32,
    banks: u32,
    data_width: u32,
    addr_width: u32,
    producer_view: bool,
    consumer_view: bool,
}

/// The frame channel `fifo_name` of `task` is, or `None` for a FIFO: a
/// stream of `task` with a frame port at either end, directly or through
/// an upper task. Graph validation has checked that the ends' frames agree;
/// the banks must fit the `FRAME_BANK_BITS` token.
pub(super) fn frame_channel(
    graph: &TaskGraph,
    modules: &ModuleTable<'_>,
    task: &Task,
    fifo_name: &str,
    fifo: &InterconnectDefinition,
) -> Result<Option<FrameChannel>, CodegenError> {
    let ends = [fifo.produced_by.as_ref(), fifo.consumed_by.as_ref()]
        .map(|end| graph.fifo_endpoint(task, fifo_name, end));
    let [producer, consumer] = [view(graph, ends[0].as_ref()), view(graph, ends[1].as_ref())];
    let Some((binding, frame)) = producer.or(consumer) else {
        return Ok(None);
    };
    // A stream the task takes rather than declares has no depth.
    let depth = fifo.depth.unwrap_or(0);
    let width = binding.port.map_or(0, |port| port.width);
    let ports: Vec<FramePort<'_>> = [(producer, &ends[0]), (consumer, &ends[1])]
        .into_iter()
        .filter_map(|(binding, end)| {
            let end = end.as_ref()?;
            Some(FramePort {
                port: binding?.0.port_name,
                module: modules.get(&end.reference.0),
            })
        })
        .collect();
    let rtl = |suffix: &str| ports.iter().filter_map(|port| port.rtl_width(suffix)).max();
    let data_width = rtl("_buf_d0").or_else(|| rtl("_buf_q0")).unwrap_or(width);
    let addr_width = rtl("_buf_address0")
        .unwrap_or_else(|| frame_signal_width(FrameSignal::Address, width, frame));
    let banks = (depth / frame).max(2);
    if banks > 1 << FRAME_BANK_BITS {
        return Err(CodegenError::FrameChannel {
            fifo: fifo_name.to_owned(),
            problem: format!(
                "its depth {depth} holds {banks} frames of {frame}, and a frame \
                 channel holds at most {}",
                1u32 << FRAME_BANK_BITS
            ),
        });
    }
    Ok(Some(FrameChannel {
        frame,
        banks,
        data_width,
        addr_width,
        producer_view: producer.is_some(),
        consumer_view: consumer.is_some(),
    }))
}

/// The binding at one end of a stream and its frame, if that end moves
/// frames by view (see [`TaskGraph::view_frame`]).
fn view<'a>(graph: &TaskGraph, end: Option<&FifoEndpoint<'a>>) -> Option<(PortBinding<'a>, u32)> {
    let end = end?;
    let binding = end.binding?;
    let frame = graph.view_frame(&end.reference.0, binding.port_name)?;
    Some((binding, frame))
}

/// The wire joining one frame-port signal of the channel `fifo`'s producer
/// (`w`) or consumer (`r`) to the channel.
pub(super) fn frame_wire(fifo: &str, cat: ArgCategory, suffix: &str) -> String {
    let side = if cat.is_output_stream() { "w" } else { "r" };
    format!("{}_{side}{suffix}", sanitize_array_name(fifo))
}

/// The `frame_channel` instance of the stream `name`.
pub(super) fn build_frame_channel_instance(name: &str, channel: &FrameChannel) -> ModuleInstance {
    let base = sanitize_array_name(name);
    let mut ports = vec![
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("reset", Expr::ident(HANDSHAKE_RST)),
    ];
    for (view, cat, stream_suffixes) in [
        (
            channel.producer_view,
            ArgCategory::Oframes,
            OSTREAM_SUFFIXES,
        ),
        (
            channel.consumer_view,
            ArgCategory::Iframes,
            ISTREAM_SUFFIXES,
        ),
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
                ports.push(PortArg::new(port, Expr::ident(format!("{base}{suffix}"))));
            } else if matches!(*suffix, "_din" | "_write" | "_read") {
                ports.push(PortArg::new(port, Expr::lit("'d0")));
            }
        }
    }
    ModuleInstance::new("frame_channel", format!("{base}_fifo"))
        .with_params(vec![
            ParamArg::new("DATA_WIDTH", Expr::int(u64::from(channel.data_width))),
            ParamArg::new("FRAME", Expr::int(u64::from(channel.frame))),
            ParamArg::new("BANKS", Expr::int(u64::from(channel.banks))),
            ParamArg::new("ADDR_WIDTH", Expr::int(u64::from(channel.addr_width))),
            ParamArg::new("BANK_WIDTH", Expr::int(u64::from(FRAME_BANK_BITS))),
            ParamArg::new("W_VIEW", Expr::int(u64::from(channel.producer_view))),
            ParamArg::new("R_VIEW", Expr::int(u64::from(channel.consumer_view))),
        ])
        .with_ports(ports)
}

/// Declare the wires between the frame channel `name` and its two ends.
pub(super) fn declare_frame_channel_wires(
    mm: &mut MutableModule,
    name: &str,
    channel: &FrameChannel,
) {
    let base = sanitize_array_name(name);
    let msb = |width: u32| (width - 1).to_string();
    for (view, cat, stream_suffixes) in [
        (
            channel.producer_view,
            ArgCategory::Oframes,
            OSTREAM_SUFFIXES,
        ),
        (
            channel.consumer_view,
            ArgCategory::Iframes,
            ISTREAM_SUFFIXES,
        ),
    ] {
        if view {
            for &(suffix, _, signal) in frame_signals(cat.is_output_stream()) {
                let width = if signal == FrameSignal::Address {
                    channel.addr_width
                } else {
                    frame_signal_width(signal, channel.data_width, channel.frame)
                };
                let wire_name = frame_wire(name, cat, suffix);
                let _ = mm.add_signal(if width > 1 {
                    wide_wire(&wire_name, &msb(width), "0")
                } else {
                    wire(&wire_name)
                });
            }
        } else {
            for suffix in stream_suffixes {
                let wire_name = format!("{base}{suffix}");
                let _ = mm.add_signal(if matches!(*suffix, "_din" | "_dout") {
                    wide_wire(&wire_name, &channel.data_width.to_string(), "0")
                } else {
                    wire(&wire_name)
                });
            }
        }
    }
}

/// Join the element side of the channel `name` on the task's own stream
/// port: the port's FIFO side (`{name}_dout`/`_empty_n`/`_read` in,
/// `_din`/`_full_n`/`_write` out) and the channel's opposite side move an
/// element whenever one is there and the other has room.
pub(super) fn join_outer_side(mm: &mut MutableModule, name: &str) {
    let base = sanitize_array_name(name);
    let signal = |suffix: &str| Expr::ident(format!("{base}{suffix}"));
    let moves = || Expr::logical_and(signal("_empty_n"), signal("_full_n"));
    mm.add_assign(ContinuousAssign::new(signal("_din"), signal("_dout")));
    mm.add_assign(ContinuousAssign::new(signal("_write"), moves()));
    mm.add_assign(ContinuousAssign::new(signal("_read"), moves()));
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
