//! The AI Engine partition of a task graph.
//!
//! Tasks tagged `synth: aie` form one ADF graph. tapacc has already resolved
//! the attributes, the `--target TASK:aie` mappings and the propagation down
//! an upper task, so each leaf's tag says where it runs. An upper task whose
//! subtree mixes PL and AI Engine leaves is first inlined into its parent
//! (see [`by_fabric`]), which leaves every PL↔AI Engine stream a FIFO the
//! top declares. [`AiePartition::derive`] then flattens the AI Engine tasks
//! into kernels and classifies every stream they touch: a FIFO between two AI
//! Engine tasks is an AI Engine net, and a FIFO between a PL task and an AI
//! Engine task is a PLIO.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::instance::{ArgSource, WireValue};
use crate::interconnect::{EndpointRef, InterconnectDefinition};
use crate::port::{sanitize_identifier_name, ArgCategory};
use crate::synth_target::SynthTarget;
use crate::task::{Task, TaskLevel};
use crate::TaskGraph;

/// Why a task graph has no valid AI Engine partition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AieError(pub String);

/// The AI Engine half of a design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiePartition {
    /// One per AI Engine leaf instance, in task-graph order.
    pub kernels: Vec<AieKernel>,
    /// The streams the kernels read and write.
    pub streams: Vec<AieStream>,
    /// Some kernel is invoked detached and never returns: the graph runs
    /// until the host ends it.
    pub run_forever: bool,
}

/// One AI Engine leaf instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieKernel {
    /// Identifier unique in the partition: the sanitized instance path.
    pub name: String,
    /// The task graph name of the task.
    pub task: String,
    /// The C++ spelling that names the task function (its readable name).
    pub call: String,
    /// The task's parameters, in declaration order.
    pub ports: Vec<AieKernelPort>,
    /// A `tapa analyze --aie-location` tile pin.
    pub location: Option<(u32, u32)>,
    /// The bytes of the task's local variables, which the kernel's stack
    /// must hold on top of its own needs.
    pub stack_bytes: Option<u32>,
    /// A cycle of streams runs through the kernel, so its stream accesses
    /// must keep program order: the AI Engine compiler may otherwise issue
    /// a read ahead of an earlier write, which the cycle turns into a
    /// deadlock.
    pub ordered: bool,
}

/// One parameter of an AI Engine kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieKernelPort {
    /// The task's parameter name.
    pub name: String,
    pub kind: AiePortKind,
}

/// What a kernel parameter carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiePortKind {
    /// A stream port: one stream, or one per channel of an `istreams` /
    /// `ostreams` port (`plural`).
    Streams {
        input: bool,
        plural: bool,
        /// Indices into [`AiePartition::streams`].
        streams: Vec<usize>,
        /// 32-bit words per element.
        words: u32,
        /// `[[tapa::frame(N)]]` elements per frame.
        frame: Option<u32>,
    },
    /// A scalar, read at run time or fixed at the invoke site.
    Scalar {
        ctype: String,
        width: u32,
        value: ScalarBinding,
    },
    /// An `async_mmap` port, served from the PL over PLIO.
    Memory(AieMemoryPort),
}

impl AiePortKind {
    /// The indices into [`AiePartition::streams`] the parameter reads or
    /// writes: its stream channels, or every channel of a memory port.
    #[must_use]
    pub fn streams(&self) -> Vec<usize> {
        match self {
            Self::Streams { streams, .. } => streams.clone(),
            Self::Scalar { .. } => Vec::new(),
            Self::Memory(memory) => {
                let reads = memory.reads.iter().flat_map(|r| [r.requests, r.data]);
                let writes = memory
                    .writes
                    .iter()
                    .flat_map(|w| std::iter::once(w.requests).chain(w.responses));
                reads.chain(writes).collect()
            }
        }
    }
}

/// An AI Engine task's `async_mmap` port: the PLIO channels of each
/// direction its body uses, which PL movers serve from a memory of the top.
/// The fields of the directions index [`AiePartition::streams`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieMemoryPort {
    /// The top task's memory argument the port is bound to.
    pub memory: String,
    pub reads: Option<AieMemoryReads>,
    pub writes: Option<AieMemoryWrites>,
}

/// The reads of an [`AieMemoryPort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieMemoryReads {
    /// The PL task serving them.
    pub mover: String,
    /// Requests, from the AI Engine to the PL.
    pub requests: usize,
    /// Data, from the PL to the AI Engine.
    pub data: usize,
}

/// The writes of an [`AieMemoryPort`]. A kernel has two output stream ports,
/// so each write request and then its data share one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieMemoryWrites {
    /// The PL task serving them.
    pub mover: String,
    /// Requests, each followed by its data, from the AI Engine to the PL.
    pub requests: usize,
    /// Acknowledgements, from the PL to the AI Engine, when the body reads
    /// them.
    pub responses: Option<usize>,
}

/// The ports of the movers (tapa/base/mover.h): `serve_reads(mem,
/// requests, data)` and `serve_writes(mem, requests[, responses])`.
const MOVER_MEMORY: &str = "mem";
const MOVER_REQUESTS: &str = "requests";
const MOVER_DATA: &str = "data";
const MOVER_RESPONSES: &str = "responses";

/// 32-bit words of a `tapa::burst` request: two `int64_t`.
const BURST_WORDS: u32 = 4;

/// Where a scalar parameter's value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarBinding {
    /// The top task's scalar argument of this name, written by the host as
    /// a runtime parameter.
    Runtime(String),
    /// An invoke-site constant, compiled into the kernel.
    Constant(WireValue),
}

/// A stream between two kernels, or between a kernel and the PL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieStream {
    /// Identifier: for a PLIO, the PLIO logical name, which is also the name
    /// of the PL kernel's AXIS port; for a net, the sanitized FIFO path.
    pub name: String,
    /// The FIFO's name in the task that declares it.
    pub fifo: String,
    /// 32-bit words per element.
    pub words: u32,
    /// Elements per frame on the AIE side, or `None` for a stream port.
    pub frame: Option<u32>,
    pub kind: AieStreamKind,
}

/// The two kinds of AI Engine stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AieStreamKind {
    /// A PL↔AI Engine stream: `input` when the PL produces it.
    Plio { input: bool, width: u32 },
    /// An AI Engine↔AI Engine stream of `depth_words` 32-bit words.
    Net { depth_words: u32 },
}

/// Widest boundary element: one 128-bit PLIO beat.
const MAX_PLIO_BITS: u32 = 128;

/// A node of the stream graph cycles are found on: a kernel, or an instance
/// of a PL task the top invokes (a PL upper task counts as one node).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Kernel(usize),
    Pl(String, u32),
}

impl AiePartition {
    /// The AI Engine partition of `graph`, or `None` when no task is on the
    /// AI Engine.
    pub fn derive(graph: &TaskGraph) -> Result<Option<Self>, AieError> {
        if !graph
            .tasks
            .values()
            .any(|task| task.synth == SynthTarget::Aie)
        {
            return Ok(None);
        }
        let graph = &*by_fabric(graph)?;
        let top = task(graph, &graph.top)?;
        let mut builder = Builder {
            graph,
            partition: Self {
                kernels: Vec::new(),
                streams: Vec::new(),
                run_forever: false,
            },
            top_streams: BTreeMap::new(),
        };
        for (child, instances) in &top.tasks {
            if task(graph, child)?.synth != SynthTarget::Aie {
                continue;
            }
            for (index, instance) in instances.iter().enumerate() {
                check_invoke_step(child, instance.step, &mut builder.partition.run_forever)?;
                let name = instance.canonical_name(child, index).into_owned();
                let mut scope = BTreeMap::new();
                for (port, arg) in &instance.args {
                    scope.insert(port.clone(), builder.top_binding(child, port, arg)?);
                }
                builder.visit(child, index, &sanitize_identifier_name(&name), &scope)?;
            }
        }
        builder.check_locations()?;
        builder.partition.mark_cycles(graph, top);
        Ok(Some(builder.partition))
    }

    /// The PL half of `graph`, the design `tapa synth` builds as RTL: the AI
    /// Engine tasks and their instances are gone, a net the top declares is
    /// gone, and every PLIO becomes a top-level stream port of the top task,
    /// named after its FIFO, carrying `aie_plio`. Codegen gives such a port
    /// its AXIS interface; `v++` wires it to the PLIO of the same name.
    /// The projected top must keep a scalar or memory argument: only those
    /// carry the `s_axi_control` interface, and a stream-only top packages
    /// as `ap_ctrl_none`, which XRT can neither start nor wait on.
    pub fn pl_projection(&self, graph: &TaskGraph) -> Result<TaskGraph, AieError> {
        let graph = &*by_fabric(graph)?;
        let mut pl = graph.clone();
        pl.tasks.retain(|_, task| task.synth != SynthTarget::Aie);
        let pl_tasks: BTreeSet<String> = pl.tasks.keys().cloned().collect();
        let original = task(graph, &graph.top)?;
        let top = pl
            .tasks
            .get_mut(&graph.top)
            .ok_or_else(|| AieError(format!("the task graph has no task `{}`", graph.top)))?;
        top.tasks.retain(|child, _| pl_tasks.contains(child));
        for stream in &self.streams {
            let (input, width) = match stream.kind {
                AieStreamKind::Plio { input, width } => (input, width),
                AieStreamKind::Net { .. } => {
                    let top_aie_net = top.fifos.get(&stream.fifo).is_some_and(|fifo| {
                        [&fifo.produced_by, &fifo.consumed_by]
                            .iter()
                            .all(|endpoint| endpoint_is_aie(graph, endpoint.as_ref()))
                    });
                    if top_aie_net {
                        top.fifos.remove(&stream.fifo);
                    }
                    continue;
                }
            };
            let Some(fifo) = top.fifos.get_mut(&stream.fifo) else {
                continue;
            };
            fifo.depth = None;
            let endpoint = if input {
                fifo.consumed_by = None;
                fifo.produced_by.clone()
            } else {
                fifo.produced_by = None;
                fifo.consumed_by.clone()
            };
            let endpoint = endpoint.ok_or_else(|| {
                AieError(format!(
                    "the PL side of stream `{}` is not connected",
                    stream.fifo
                ))
            })?;
            let port_name =
                child_port_for(original, &endpoint.0, endpoint.1, &stream.fifo).unwrap_or_default();
            let pl_port = task(graph, &endpoint.0)?.port(&port_name).ok_or_else(|| {
                AieError(format!(
                    "the task `{}` has no port `{port_name}`",
                    endpoint.0
                ))
            })?;
            top.ports.push(crate::Port {
                cat: if input {
                    ArgCategory::Ostream
                } else {
                    ArgCategory::Istream
                },
                name: stream.fifo.clone(),
                ctype: pl_port.ctype.clone(),
                width: pl_port.width,
                chan_count: None,
                chan_size: None,
                frame: None,
                aie_plio: Some(width),
                stream_depth: None,
                mmap_addr_width: None,
                aie_movers: None,
            });
        }
        for kernel in &self.kernels {
            for port in &kernel.ports {
                if let AiePortKind::Memory(memory) = &port.kind {
                    self.add_movers(graph, original, top, &kernel.name, &port.name, memory)?;
                }
            }
        }
        // The same rule `kernel.xml` applies: `has_s_axi_control` exists only
        // beside a scalar or memory argument, never a stream.
        if !top
            .ports
            .iter()
            .any(|port| port.cat.is_scalar() || port.cat.is_mmap_like())
        {
            return Err(AieError(format!(
                "the top task `{}` keeps only stream ports beside the AI Engine graph; a \
                 hybrid top needs at least one scalar or memory-mapped argument so Vitis \
                 emits `ap_ctrl_hs` and XRT can start and wait on the PL kernel, not the \
                 `ap_ctrl_none` of a free-running one",
                graph.top
            )));
        }
        Ok(pl)
    }

    /// The PL movers of one kernel's `async_mmap` port, invoked by the top
    /// on the port's memory, and the top's AXIS ports for its channels.
    fn add_movers(
        &self,
        graph: &TaskGraph,
        original: &Task,
        top: &mut Task,
        kernel: &str,
        port: &str,
        memory: &AieMemoryPort,
    ) -> Result<(), AieError> {
        // Nothing orders an AI Engine task's accesses against a PL task's.
        for (child, instances) in &original.tasks {
            if task(graph, child)?.synth == SynthTarget::Aie {
                continue;
            }
            let shares = instances.iter().any(|instance| {
                instance.args.values().any(|arg| {
                    arg.cat.is_mmap_like()
                        && matches!(&arg.arg, ArgSource::Name(name) if *name == memory.memory)
                })
            });
            if shares {
                return Err(AieError(format!(
                    "the memory `{}` is bound to the async_mmap port `{port}` of the AI Engine \
                     kernel `{kernel}` and to the PL task `{child}`; nothing orders their \
                     accesses, so give each its own memory",
                    memory.memory
                )));
            }
        }
        let reads = memory.reads.as_ref().map(|reads| {
            let ports = vec![(MOVER_REQUESTS, reads.requests), (MOVER_DATA, reads.data)];
            ("reads", &reads.mover, ports)
        });
        let writes = memory.writes.as_ref().map(|writes| {
            let mut ports = vec![(MOVER_REQUESTS, writes.requests)];
            ports.extend(writes.responses.map(|stream| (MOVER_RESPONSES, stream)));
            ("writes", &writes.mover, ports)
        });
        for (direction, mover_name, ports) in reads.into_iter().chain(writes) {
            let mover = task(graph, mover_name)?;
            let instance = top.tasks.get(mover_name).map_or(0, Vec::len);
            let endpoint = EndpointRef(
                mover_name.clone(),
                u32::try_from(instance).unwrap_or(u32::MAX),
            );
            let mut args = BTreeMap::new();
            args.insert(
                MOVER_MEMORY.to_string(),
                crate::Arg::named(memory.memory.clone(), ArgCategory::AsyncMmap),
            );
            for (mover_port, stream) in ports {
                let stream = &self.streams[stream];
                let AieStreamKind::Plio { input, width } = stream.kind else {
                    continue;
                };
                let pl_port = mover.port(mover_port).ok_or_else(|| {
                    AieError(format!(
                        "the mover `{mover_name}` has no port `{mover_port}`"
                    ))
                })?;
                top.ports.push(crate::Port {
                    cat: if input {
                        ArgCategory::Ostream
                    } else {
                        ArgCategory::Istream
                    },
                    name: stream.name.clone(),
                    ctype: pl_port.ctype.clone(),
                    width: pl_port.width,
                    chan_count: None,
                    chan_size: None,
                    frame: None,
                    aie_plio: Some(width),
                    stream_depth: None,
                    mmap_addr_width: None,
                    aie_movers: None,
                });
                args.insert(
                    mover_port.to_string(),
                    crate::Arg::named(stream.name.clone(), pl_port.cat),
                );
                // Codegen gives the port its AXIS interface through an
                // external FIFO, as for any PLIO of the projection.
                top.fifos.insert(
                    stream.name.clone(),
                    InterconnectDefinition {
                        depth: None,
                        produced_by: input.then(|| endpoint.clone()),
                        consumed_by: (!input).then(|| endpoint.clone()),
                    },
                );
            }
            top.tasks
                .entry(mover_name.clone())
                .or_default()
                .push(crate::TaskInstance {
                    name: Some(sanitize_identifier_name(&format!(
                        "{kernel}__{port}_{direction}"
                    ))),
                    args,
                    step: 0,
                    position: 0,
                });
        }
        Ok(())
    }

    /// Sets [`AieKernel::ordered`] on every kernel a stream cycle runs
    /// through, by way of other kernels or the top's PL instances.
    fn mark_cycles(&mut self, graph: &TaskGraph, top: &Task) {
        let pl = |end: Option<&EndpointRef>| {
            end.filter(|end| {
                graph
                    .tasks
                    .get(&end.0)
                    .is_some_and(|t| t.synth != SynthTarget::Aie)
            })
            .map(|end| Node::Pl(end.0.clone(), end.1))
        };
        let mut ends = vec![(None, None); self.streams.len()];
        for (index, kernel) in self.kernels.iter().enumerate() {
            for port in &kernel.ports {
                if let AiePortKind::Streams { input, streams, .. } = &port.kind {
                    for &stream in streams {
                        let (producer, consumer) = &mut ends[stream];
                        *if *input { consumer } else { producer } = Some(Node::Kernel(index));
                    }
                }
            }
        }
        let mut edges: BTreeMap<Node, BTreeSet<Node>> = BTreeMap::new();
        for (stream, (producer, consumer)) in self.streams.iter().zip(ends) {
            let fifo = top.fifos.get(&stream.fifo);
            let producer = producer.or_else(|| pl(fifo.and_then(|f| f.produced_by.as_ref())));
            let consumer = consumer.or_else(|| pl(fifo.and_then(|f| f.consumed_by.as_ref())));
            if let (Some(from), Some(to)) = (producer, consumer) {
                edges.entry(from).or_default().insert(to);
            }
        }
        for fifo in top.fifos.values() {
            if let (Some(from), Some(to)) =
                (pl(fifo.produced_by.as_ref()), pl(fifo.consumed_by.as_ref()))
            {
                edges.entry(from).or_default().insert(to);
            }
        }
        for (index, kernel) in self.kernels.iter_mut().enumerate() {
            // A memory port's requests and data are a round trip through
            // its PL mover, the same as a cycle.
            let memory = kernel
                .ports
                .iter()
                .any(|port| matches!(port.kind, AiePortKind::Memory(_)));
            kernel.ordered = memory || reaches_itself(&edges, &Node::Kernel(index));
        }
    }
}

/// An upper task instance dissolved into its parent for running on both
/// fabrics.
///
/// The partition and the PL projection inline it. A host program running
/// the design against one fabric runs the instance's body itself, so it
/// has to find the instance among its parent's invokes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InlinedInstance {
    /// The inlined instance that invokes it; empty for the top.
    pub parent: String,
    /// Where it sits in the parent's chain of invokes.
    pub position: u32,
    /// Its name once inlined, which prefixes the names of its streams.
    pub name: String,
}

/// Every instance the partition inlines (see [`InlinedInstance`]), each
/// after its parent.
///
/// # Errors
///
/// A parent whose invokes do not have distinct positions: the task graph
/// is older than invoke positions.
pub fn inlined_instances(graph: &TaskGraph) -> Result<Vec<InlinedInstance>, AieError> {
    let mixed = mixed_uppers(graph);
    let mut inlined = Vec::new();
    let mut scopes = vec![(String::new(), graph.top.as_str())];
    while let Some((scope, task)) = scopes.pop() {
        let Some(def) = graph.tasks.get(task) else {
            continue;
        };
        let mut positions = BTreeSet::new();
        let total: usize = def.tasks.values().map(Vec::len).sum();
        positions.extend(
            def.tasks
                .values()
                .flatten()
                .map(|instance| instance.position),
        );
        let mut found = false;
        for (child, instances) in def
            .tasks
            .iter()
            .filter(|(child, _)| mixed.contains(child.as_str()))
        {
            for (index, instance) in instances.iter().enumerate() {
                let local = instance.canonical_name(child, index);
                let name = if scope.is_empty() {
                    local.into_owned()
                } else {
                    format!("{scope}__{local}")
                };
                inlined.push(InlinedInstance {
                    parent: scope.clone(),
                    position: instance.position,
                    name: name.clone(),
                });
                scopes.push((name, child.as_str()));
                found = true;
            }
        }
        if found && positions.len() != total {
            return Err(AieError(format!(
                "`{task}` has invokes at the same position; re-run `tapa analyze` so the task \
                 graph records where each invoke sits"
            )));
        }
    }
    Ok(inlined)
}

/// An `async_mmap` port of an AI Engine task as the host program reaches it.
///
/// The invoke is named by its parent and position as in
/// [`InlinedInstance`]; the channels are the PLIOs of each direction the
/// body uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryCrossing {
    /// The inlined instance whose body makes the invoke, or empty for the top.
    pub parent: String,
    /// The invoke's position in that body.
    pub position: u32,
    /// The parameter's index in the task's parameters.
    pub param: u32,
    /// The read requests, from the AI Engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_requests: Option<String>,
    /// The read data, to the AI Engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_data: Option<String>,
    /// The write requests with their data, from the AI Engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_requests: Option<String>,
    /// The write acknowledgements, to the AI Engine, when the body reads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_responses: Option<String>,
}

/// Every `async_mmap` port of an AI Engine task the host program invokes
/// from the top or from an upper task the partition inlines.
///
/// # Errors
///
/// No valid partition, or an `async_mmap` an AI Engine upper task hands on
/// to its tasks: a host program cannot tell which of them a request is for.
pub fn memory_crossings(graph: &TaskGraph) -> Result<Vec<MemoryCrossing>, AieError> {
    let Some(partition) = AiePartition::derive(graph)? else {
        return Ok(Vec::new());
    };
    // Kernel name -> where the host invokes it, walking the bodies the host
    // records: the top's and every inlined upper task's.
    let mixed = mixed_uppers(graph);
    let mut sites = BTreeMap::new();
    let mut scopes = vec![(String::new(), graph.top.as_str())];
    while let Some((scope, task_name)) = scopes.pop() {
        let def = task(graph, task_name)?;
        for (child, instances) in &def.tasks {
            for (index, instance) in instances.iter().enumerate() {
                let local = instance.canonical_name(child, index);
                let name = if scope.is_empty() {
                    local.into_owned()
                } else {
                    format!("{scope}__{local}")
                };
                if mixed.contains(child.as_str()) {
                    scopes.push((name, child.as_str()));
                } else if task(graph, child)?.synth == SynthTarget::Aie {
                    sites.insert(
                        sanitize_identifier_name(&name),
                        (scope.clone(), instance.position),
                    );
                }
            }
        }
    }
    let name = |index: usize| partition.streams[index].name.clone();
    let mut crossings = Vec::new();
    for kernel in &partition.kernels {
        for (param, port) in kernel.ports.iter().enumerate() {
            let AiePortKind::Memory(memory) = &port.kind else {
                continue;
            };
            let Some((parent, position)) = sites.get(&kernel.name) else {
                return Err(AieError(format!(
                    "the AI Engine kernel `{}` takes its memory `{}` from an AI Engine upper \
                     task, which the host program cannot serve in simulation; invoke the \
                     task that takes the async_mmap from a PL task or the top",
                    kernel.name, port.name
                )));
            };
            crossings.push(MemoryCrossing {
                parent: parent.clone(),
                position: *position,
                param: u32::try_from(param).unwrap_or(u32::MAX),
                read_requests: memory.reads.as_ref().map(|r| name(r.requests)),
                read_data: memory.reads.as_ref().map(|r| name(r.data)),
                write_requests: memory.writes.as_ref().map(|w| name(w.requests)),
                write_responses: memory.writes.as_ref().and_then(|w| w.responses).map(name),
            });
        }
    }
    Ok(crossings)
}

/// The upper tasks below the top whose leaves are not all on their own
/// fabric.
fn mixed_uppers(graph: &TaskGraph) -> BTreeSet<&str> {
    let mut leaves = BTreeMap::new();
    graph
        .tasks
        .iter()
        .filter(|(name, def)| *name != &graph.top && def.level == TaskLevel::Upper)
        .filter(|(name, def)| {
            let aie = def.synth == SynthTarget::Aie;
            leaf_fabrics(graph, name, &mut leaves)
                .iter()
                .any(|&leaf| leaf != aie)
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

/// `graph` with every non-top upper task whose leaves are not all on its own
/// fabric inlined into its parent, so each remaining upper task below the
/// top is purely PL or purely AI Engine. An upper task's tag is only the
/// default its leaves inherit; the leaves decide where the work runs.
fn by_fabric(graph: &TaskGraph) -> Result<Cow<'_, TaskGraph>, AieError> {
    let mixed = mixed_uppers(graph);
    if mixed.is_empty() {
        return Ok(Cow::Borrowed(graph));
    }
    crate::transforms::inline_uppers(graph, |name| mixed.contains(name))
        .map(Cow::Owned)
        .map_err(|error| {
            AieError(format!(
                "{error}; an upper task whose subtree mixes PL and AI Engine tasks is inlined \
                 into its parent, so it and its children must be invoked joined or detached"
            ))
        })
}

/// Whether each leaf below `name` (or `name` itself, for a leaf) is on the
/// AI Engine; memoized in `memo`.
fn leaf_fabrics(
    graph: &TaskGraph,
    name: &str,
    memo: &mut BTreeMap<String, BTreeSet<bool>>,
) -> BTreeSet<bool> {
    if let Some(known) = memo.get(name) {
        return known.clone();
    }
    let Some(def) = graph.tasks.get(name) else {
        return BTreeSet::new();
    };
    let fabrics = if def.level == TaskLevel::Lower {
        BTreeSet::from([def.synth == SynthTarget::Aie])
    } else {
        let mut all = BTreeSet::new();
        for child in def.tasks.keys() {
            all.extend(leaf_fabrics(graph, child, memo));
        }
        all
    };
    memo.insert(name.to_string(), fabrics.clone());
    fabrics
}

fn reaches_itself(edges: &BTreeMap<Node, BTreeSet<Node>>, start: &Node) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&Node> = edges.get(start).into_iter().flatten().collect();
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if seen.insert(node) {
            stack.extend(edges.get(node).into_iter().flatten());
        }
    }
    false
}

fn task<'g>(graph: &'g TaskGraph, name: &str) -> Result<&'g Task, AieError> {
    graph
        .tasks
        .get(name)
        .ok_or_else(|| AieError(format!("the task graph has no task `{name}`")))
}

/// AI Engine kernels run concurrently: a sequential invoke is an error, and a
/// detached invoke means the graph runs until the host ends it.
fn check_invoke_step(child: &str, step: i64, run_forever: &mut bool) -> Result<(), AieError> {
    if step > 0 {
        return Err(AieError(format!(
            "the AI Engine task `{child}` is invoked sequentially (step {step}); \
             AI Engine kernels run concurrently, so invoke it joined or detached"
        )));
    }
    *run_forever |= step < 0;
    Ok(())
}

/// Whether the endpoint resolves to a task synthesized for the AI Engine.
fn endpoint_is_aie(graph: &TaskGraph, endpoint: Option<&EndpointRef>) -> bool {
    endpoint
        .and_then(|e| graph.tasks.get(&e.0))
        .is_some_and(|t| t.synth == SynthTarget::Aie)
}

/// What a name in a task's scope is bound to.
#[derive(Debug, Clone)]
enum Binding {
    Stream(usize),
    Scalar(ScalarBinding),
    /// A memory argument of the top, bound to an `async_mmap` port.
    Memory(String),
    /// That memory, once a leaf's port has its PLIO channels.
    MemoryPort(AieMemoryPort),
}

struct Builder<'g> {
    graph: &'g TaskGraph,
    partition: AiePartition,
    /// Top-level FIFO name → stream index, created on first use.
    top_streams: BTreeMap<String, usize>,
}

impl Builder<'_> {
    /// What one argument of a top-level AI Engine invoke is bound to.
    fn top_binding(
        &mut self,
        child: &str,
        port: &str,
        arg: &crate::instance::Arg,
    ) -> Result<Binding, AieError> {
        let graph = self.graph;
        let top = task(graph, &graph.top)?;
        match &arg.arg {
            ArgSource::Literal(value) => Ok(Binding::Scalar(ScalarBinding::Constant(*value))),
            ArgSource::Name(name) if arg.cat.is_scalar() => {
                Ok(Binding::Scalar(ScalarBinding::Runtime(name.clone())))
            }
            ArgSource::Name(name) if arg.cat.is_stream() => {
                if let Some(index) = self.top_streams.get(name) {
                    return Ok(Binding::Stream(*index));
                }
                let fifo = top.fifos.get(name).filter(|fifo| fifo.depth.is_some());
                let Some(fifo) = fifo else {
                    return Err(AieError(format!(
                        "port `{port}` of the AI Engine task `{child}` is bound to the top \
                         task's port `{name}`; connect an AI Engine task to the PL through a \
                         stream the top declares"
                    )));
                };
                let producer_aie = endpoint_is_aie(graph, fifo.produced_by.as_ref());
                let consumer_aie = endpoint_is_aie(graph, fifo.consumed_by.as_ref());
                let words = stream_words(graph, child, port)?;
                let aie_port = task(graph, child)?.port(port);
                let frame = aie_port.and_then(|port| port.frame);
                let kind = if producer_aie && consumer_aie {
                    AieStreamKind::Net {
                        depth_words: fifo.depth.unwrap_or(0) * words,
                    }
                } else {
                    let width = aie_port.and_then(|p| p.aie_plio).unwrap_or(words * 32);
                    let bits = words * 32;
                    if bits > MAX_PLIO_BITS {
                        return Err(AieError(format!(
                            "stream `{name}` between the PL and the AI Engine task `{child}` \
                             carries {bits}-bit elements; a PL↔AI Engine stream carries at most \
                             {MAX_PLIO_BITS} bits per element, so split it into \
                             tapa::vec_t<T, 4> elements of 32-bit T"
                        )));
                    }
                    AieStreamKind::Plio {
                        input: !producer_aie,
                        width: plio_width(width),
                    }
                };
                let index =
                    self.push_stream(sanitize_identifier_name(name), name, words, frame, kind);
                self.top_streams.insert(name.clone(), index);
                Ok(Binding::Stream(index))
            }
            ArgSource::Name(name)
                if arg.cat == ArgCategory::AsyncMmap
                    && top.port(name).is_some_and(|p| p.cat.is_mmap_like()) =>
            {
                Ok(Binding::Memory(name.clone()))
            }
            ArgSource::Name(name) => Err(AieError(format!(
                "port `{port}` of the AI Engine task `{child}` is bound to the memory `{name}`; \
                 an AI Engine task reaches memory only through a `tapa::async_mmap<T>&` port \
                 bound to a memory argument of the top"
            ))),
        }
    }

    /// The PLIO channels of a leaf's `async_mmap` port, one set per
    /// direction its body uses, named after the kernel and the port.
    fn memory_port(
        &mut self,
        task_name: &str,
        path: &str,
        port: &crate::Port,
        memory: String,
    ) -> Result<AieMemoryPort, AieError> {
        let movers = port.aie_movers.as_ref().ok_or_else(|| {
            AieError(format!(
                "the async_mmap port `{}` of the AI Engine task `{task_name}` names no PL \
                 movers; re-run `tapa analyze` with this version of TAPA",
                port.name
            ))
        })?;
        if ![32, 64, 128].contains(&port.width) {
            return Err(AieError(format!(
                "the async_mmap port `{}` of the AI Engine task `{task_name}` has {}-bit \
                 elements; an AI Engine memory element is 32, 64 or 128 bits, the widths \
                 that cross a PLIO and line up with a 128-bit request",
                port.name, port.width
            )));
        }
        let words = port.width / 32;
        let graph = self.graph;
        let mut channel = |channel: &str, input: bool, words: u32| {
            let name = sanitize_identifier_name(&format!("{path}__{}_{channel}", port.name));
            let kind = AieStreamKind::Plio {
                input,
                width: plio_width(words * 32),
            };
            self.push_stream(name.clone(), &name, words, None, kind)
        };
        let reads = movers.reads.as_ref().map(|mover| AieMemoryReads {
            mover: mover.clone(),
            requests: channel("read_addr", false, BURST_WORDS),
            data: channel("read_data", true, words),
        });
        let writes = match &movers.writes {
            Some(mover) => {
                let respond = task(graph, mover)?.port(MOVER_RESPONSES).is_some();
                Some(AieMemoryWrites {
                    mover: mover.clone(),
                    requests: channel("write", false, words),
                    responses: respond.then(|| channel("write_resp", true, 1)),
                })
            }
            None => None,
        };
        Ok(AieMemoryPort {
            memory,
            reads,
            writes,
        })
    }

    fn push_stream(
        &mut self,
        name: String,
        fifo: &str,
        words: u32,
        frame: Option<u32>,
        kind: AieStreamKind,
    ) -> usize {
        self.partition.streams.push(AieStream {
            name,
            fifo: fifo.to_string(),
            words,
            frame,
            kind,
        });
        self.partition.streams.len() - 1
    }

    /// Flattens one AI Engine task instance whose ports are bound by `scope`.
    fn visit(
        &mut self,
        task_name: &str,
        index: usize,
        path: &str,
        scope: &BTreeMap<String, Binding>,
    ) -> Result<(), AieError> {
        let graph = self.graph;
        let def = task(graph, task_name)?;
        if def.level == TaskLevel::Lower {
            let mut scope = Cow::Borrowed(scope);
            for port in def.ports.iter().filter(|p| p.cat == ArgCategory::AsyncMmap) {
                if let Some(Binding::Memory(memory)) = scope.get(&port.name).cloned() {
                    let bound = self.memory_port(task_name, path, port, memory)?;
                    scope
                        .to_mut()
                        .insert(port.name.clone(), Binding::MemoryPort(bound));
                }
            }
            let kernel = Self::kernel(task_name, index, path, def, &scope)?;
            self.partition.kernels.push(kernel);
            return Ok(());
        }
        // An AI Engine upper task: its FIFOs are nets; its children see
        // either those nets or the bindings of the upper task's own ports.
        let mut inner: BTreeMap<String, Binding> = BTreeMap::new();
        for (fifo_name, fifo) in &def.fifos {
            let Some(endpoint) = fifo.produced_by.as_ref().or(fifo.consumed_by.as_ref()) else {
                continue;
            };
            let port = child_port_for(def, &endpoint.0, endpoint.1, fifo_name).unwrap_or_default();
            let words = stream_words(graph, &endpoint.0, &port)?;
            let frame = task(graph, &endpoint.0)?
                .port(&port)
                .and_then(|port| port.frame);
            let stream = self.push_stream(
                sanitize_identifier_name(&format!("{path}__{fifo_name}")),
                fifo_name,
                words,
                frame,
                AieStreamKind::Net {
                    depth_words: fifo.depth.unwrap_or(0) * words,
                },
            );
            inner.insert(fifo_name.clone(), Binding::Stream(stream));
        }
        for (child, instances) in &def.tasks {
            for (child_index, instance) in instances.iter().enumerate() {
                check_invoke_step(child, instance.step, &mut self.partition.run_forever)?;
                let mut child_scope = BTreeMap::new();
                for (port, arg) in &instance.args {
                    let binding = match &arg.arg {
                        ArgSource::Literal(value) => {
                            Binding::Scalar(ScalarBinding::Constant(*value))
                        }
                        ArgSource::Name(name) => inner
                            .get(name)
                            .or_else(|| scope.get(name))
                            .cloned()
                            .ok_or_else(|| {
                                AieError(format!(
                                    "`{name}` bound to `{child}.{port}` in the AI Engine task \
                                     `{task_name}` is neither a stream it declares nor one of \
                                     its ports"
                                ))
                            })?,
                    };
                    child_scope.insert(port.clone(), binding);
                }
                let child_name = instance.canonical_name(child, child_index);
                let child_path = sanitize_identifier_name(&format!("{path}__{child_name}"));
                self.visit(child, child_index, &child_path, &child_scope)?;
            }
        }
        Ok(())
    }

    fn kernel(
        task_name: &str,
        index: usize,
        path: &str,
        def: &Task,
        scope: &BTreeMap<String, Binding>,
    ) -> Result<AieKernel, AieError> {
        let mut ports = Vec::new();
        for port in &def.ports {
            let kind = if port.cat.is_stream() {
                let plural = matches!(port.cat, ArgCategory::Istreams | ArgCategory::Ostreams);
                let channels: Vec<String> = if plural {
                    (0..port.chan_count.unwrap_or(0))
                        .map(|i| format!("{}[{i}]", port.name))
                        .collect()
                } else {
                    vec![port.name.clone()]
                };
                let mut streams = Vec::new();
                for channel in &channels {
                    match scope.get(channel) {
                        Some(Binding::Stream(stream)) => streams.push(*stream),
                        _ => {
                            return Err(AieError(format!(
                                "stream port `{channel}` of the AI Engine task `{task_name}` \
                                 is not bound to a stream"
                            )))
                        }
                    }
                }
                AiePortKind::Streams {
                    input: port.cat.is_input_stream(),
                    plural,
                    streams,
                    words: port.width.div_ceil(32),
                    frame: port.frame,
                }
            } else if port.cat.is_scalar() {
                let Some(Binding::Scalar(value)) = scope.get(&port.name) else {
                    return Err(AieError(format!(
                        "scalar port `{}` of the AI Engine task `{task_name}` is not bound to \
                         a top-task scalar or a constant",
                        port.name
                    )));
                };
                AiePortKind::Scalar {
                    ctype: port.ctype.clone(),
                    width: port.width,
                    value: value.clone(),
                }
            } else if let Some(Binding::MemoryPort(memory)) = scope.get(&port.name) {
                AiePortKind::Memory(memory.clone())
            } else {
                return Err(AieError(format!(
                    "the AI Engine task `{task_name}` takes the memory port `{}`; an AI Engine \
                     task reaches memory only through a `tapa::async_mmap<T>&` port bound to a \
                     memory argument of the top",
                    port.name
                )));
            };
            ports.push(AieKernelPort {
                name: port.name.clone(),
                kind,
            });
        }
        let location = def
            .aie_location
            .iter()
            .find(|pin| pin.instance.is_none_or(|i| i as usize == index))
            .map(|pin| (pin.col, pin.row));
        Ok(AieKernel {
            name: path.to_string(),
            task: task_name.to_string(),
            call: def.readable_name.clone(),
            ports,
            location,
            stack_bytes: def.aie_stack_bytes,
            ordered: false,
        })
    }

    /// A pin without an instance index must name a task with one instance,
    /// and no two kernels may share a tile.
    fn check_locations(&self) -> Result<(), AieError> {
        for (name, def) in &self.graph.tasks {
            let instances = self
                .partition
                .kernels
                .iter()
                .filter(|k| &k.task == name)
                .count();
            for pin in &def.aie_location {
                match pin.instance {
                    None if instances > 1 => {
                        return Err(AieError(format!(
                            "`--aie-location {name}:{},{}` names no instance, but the task \
                             has {instances}; add the instance index as a third number",
                            pin.col, pin.row
                        )))
                    }
                    Some(i) if i as usize >= instances => {
                        return Err(AieError(format!(
                            "`--aie-location {name}:{},{},{i}` names instance {i}, but the \
                             task has {instances}",
                            pin.col, pin.row
                        )))
                    }
                    _ => {}
                }
            }
        }
        let mut tiles = BTreeMap::new();
        for kernel in &self.partition.kernels {
            if let Some(tile) = kernel.location {
                if let Some(other) = tiles.insert(tile, &kernel.name) {
                    return Err(AieError(format!(
                        "the AI Engine kernels `{other}` and `{}` are both pinned to tile \
                         ({}, {})",
                        kernel.name, tile.0, tile.1
                    )));
                }
            }
        }
        Ok(())
    }
}

/// The child port of `parent`'s instance `child[index]` bound to `fifo`.
fn child_port_for(parent: &Task, child: &str, index: u32, fifo: &str) -> Option<String> {
    parent
        .tasks
        .get(child)?
        .get(index as usize)?
        .args
        .iter()
        .find(|(_, arg)| arg.name() == Some(fifo))
        .map(|(port, _)| port.clone())
}

/// 32-bit words per element of `task`'s stream port `port` (a channel name
/// like `a[2]` resolves to its plural port `a`).
fn stream_words(graph: &TaskGraph, task_name: &str, port: &str) -> Result<u32, AieError> {
    let def = task(graph, task_name)?;
    def.port(port)
        .map(|p| p.width.div_ceil(32))
        .ok_or_else(|| AieError(format!("the task `{task_name}` has no port `{port}`")))
}

/// The PLIO width carrying `bits`-bit beats: 32, 64 or 128.
const fn plio_width(bits: u32) -> u32 {
    if bits <= 32 {
        32
    } else if bits <= 64 {
        64
    } else {
        128
    }
}

#[cfg(test)]
mod tests;
