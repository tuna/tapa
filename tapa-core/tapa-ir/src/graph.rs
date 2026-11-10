//! Root task-graph container.
//!
//! `TaskGraph` is the single root type for both the `tapacc` output and the
//! post-synthesis design model; it is persisted inside the work dir's
//! `tapa.json`, which `tapa pack` copies verbatim into the `.zip` archive for
//! `frt-cosim` to read back.
//! Field declaration order is stable so serializing re-emits keys
//! deterministically; `tasks` uses [`BTreeMap`] so keys come out
//! alphabetically, matching the sorted order `tapa analyze` writes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ParseError;
use crate::port::ArgCategory;
use crate::target::Target;
use crate::task::Task;

/// The PL storage of a stream with an end that moves frames by view: one
/// memory of `banks` frames, handed between the ends in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameChannel {
    /// Elements in a frame.
    pub frame: u32,
    /// Frames the channel holds: as many as the stream's depth, and at
    /// least two, so one end fills a frame while the other drains one.
    pub banks: u32,
    /// The producer writes frames in place.
    pub producer_view: bool,
    /// The consumer reads frames in place.
    pub consumer_view: bool,
}

impl FrameChannel {
    /// Depth of the element stream that copies frames between the two
    /// halves of a channel whose ends the floorplan puts in different
    /// slots; the halves hold the frames.
    pub const COPY_DEPTH: u32 = 2;
}

/// A stream argument of a Vitis top carried by a ring of frame slots in
/// device memory, in place of an AXIS port.
///
/// The host fills (input) or empties (output) slots, and a mover in the
/// top moves each frame between its slot and the argument's frame channel.
/// Two frame counters in the control registers, after the last argument,
/// hand the slots over: frames the host has written (input) or taken
/// (output), and frames the kernel has taken (input) or written (output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    /// The top's stream port.
    pub port: String,
    /// The kernel reads the frames.
    pub input: bool,
    /// Elements in a frame.
    pub frame: u32,
    /// Frames the ring holds.
    pub slots: u32,
}

/// A control register of a ring, after the top's last argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingRegister {
    pub name: String,
    pub bits: u32,
    /// The host writes it; the kernel writes the others.
    pub host_writes: bool,
}

/// The registers of the ring of the top's port `port`.
///
/// In order: frames the host has moved, frames the kernel has moved, and
/// the ring's shape (the frame's elements in the low word, the slots in the
/// high word, whose top bit is set when the kernel reads the frames).
#[must_use]
pub fn ring_registers(port: &str) -> [RingRegister; 3] {
    let port = crate::port::sanitize_array_name(port);
    let register = |suffix: &str, bits, host_writes| RingRegister {
        name: format!("{port}_{suffix}"),
        bits,
        host_writes,
    };
    [
        register("host_frames", 32, true),
        register("kernel_frames", 32, false),
        register("ring_shape", 64, false),
    ]
}

/// Root of the unified task graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskGraph {
    /// Producer schema version, stamped by `tapacc`. Absent means a
    /// pre-versioning (v1) producer, whose invoke-site constants this
    /// schema would misread as wire names, so both directions of mismatch
    /// are rejected with a clear regenerate message instead of a
    /// field-level misparse.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Name of the top-level task.
    pub top: String,
    /// Compilation flow target, e.g. [`Target::XilinxVitis`]. Serializes as
    /// the wire string `"xilinx-vitis"` / `"xilinx-hls"`.
    pub target: Target,
    /// C++ compiler flags.
    #[serde(default)]
    pub cflags: Vec<String>,
    /// Task definitions keyed by task name.
    pub tasks: BTreeMap<String, Task>,
}

/// The task-graph schema version this crate reads and writes.
pub const SCHEMA_VERSION: u32 = 4;

/// A pre-versioning producer's graph is version 1 by definition.
const fn default_schema_version() -> u32 {
    1
}

impl TaskGraph {
    /// Parse one JSON task-graph document with field-path error diagnostics.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let graph: Self = crate::error::parse_json(json)?;
        graph.validate()?;
        Ok(graph)
    }

    /// Validate graph invariants at both frontend and persisted-state boundaries.
    ///
    /// # Errors
    ///
    /// A schema version other than [`SCHEMA_VERSION`], an invalid
    /// invoke-site constant, or a stream shallower than a frame it carries.
    pub fn validate(&self) -> Result<(), ParseError> {
        if self.schema_version > SCHEMA_VERSION {
            return Err(ParseError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        if self.schema_version < SCHEMA_VERSION {
            return Err(ParseError::OutdatedSchemaVersion {
                found: self.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        self.validate_literals()?;
        self.validate_frame_depths()?;
        self.validate_frame_channels()
    }

    /// Both ends of a stream agree on the frame where both move frames, and a
    /// frame port's stream reaches a frame channel: one its invoker declares,
    /// or one of the invoker's stream ports, which the channel is behind. A
    /// leaf top has no invoker to hold the channel.
    fn validate_frame_channels(&self) -> Result<(), ParseError> {
        let mut memo = BTreeMap::new();
        if let Some(top) = self.tasks.get(&self.top) {
            if let Some(port) = top.ports.iter().find(|port| port.cat.is_frames()) {
                return Err(ParseError::FrameChannel {
                    task: self.top.clone(),
                    stream: port.name.clone(),
                    problem: "a top task's frame port has no task to hold its frame \
                              channel; invoke the leaf from an upper top task"
                        .to_string(),
                });
            }
        }
        for (task_name, task) in &self.tasks {
            let error = |stream: &str, problem: String| ParseError::FrameChannel {
                task: task_name.clone(),
                stream: stream.to_string(),
                problem,
            };
            for binding in self.bindings(task) {
                let Some(stream) = binding.argument.name() else {
                    continue;
                };
                let declared = task
                    .fifos
                    .get(stream)
                    .is_some_and(|fifo| fifo.depth.is_some());
                let passed_on = task.port(stream).is_some_and(|port| port.cat.is_stream());
                if binding.argument.cat.is_frames() && !declared && !passed_on {
                    return Err(error(
                        stream,
                        format!(
                            "frame port `{}` must bind a stream the task declares or one of \
                             its stream ports",
                            binding.port_name
                        ),
                    ));
                }
            }
            for (stream, fifo) in &task.fifos {
                let ends = [fifo.produced_by.as_ref(), fifo.consumed_by.as_ref()]
                    .map(|end| self.fifo_endpoint(task, stream, end));
                let frames = ends.each_ref().map(|end| {
                    let end = end.as_ref()?;
                    self.port_frames(&end.reference.0, &mut memo)
                        .get(end.binding?.port_name)
                        .copied()
                });
                if let [Some(a), Some(b)] = frames {
                    if a != b {
                        return Err(error(
                            stream,
                            format!(
                                "its producer moves frames of {a} elements and its \
                                 consumer frames of {b}"
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The frame size of the port `port` of `task` when the port moves
    /// frames by view: a leaf's `iframes` / `oframes`, or an upper task's
    /// stream port bound to one below it, which then carries the view's
    /// signals through the upper task.
    #[must_use]
    pub fn view_frame(&self, task: &str, port: &str) -> Option<u32> {
        let def = self.tasks.get(task)?;
        let own = def.port(port)?;
        if own.cat.is_frames() {
            return own.frame;
        }
        if def.level != crate::task::TaskLevel::Upper
            || !matches!(own.cat, ArgCategory::Istream | ArgCategory::Ostream)
        {
            return None;
        }
        def.tasks.iter().find_map(|(child, instances)| {
            instances
                .iter()
                .flat_map(|instance| &instance.args)
                .find(|(_, arg)| arg.arg.name() == Some(port))
                .and_then(|(child_port, _)| self.view_frame(child, child_port))
        })
    }

    /// The frame channel the stream `fifo_name` of `task` becomes on the PL,
    /// or `None` for a FIFO: a stream with an end that moves frames by view
    /// (see [`Self::view_frame`]). Graph validation has checked that the
    /// ends' frames agree.
    #[must_use]
    pub fn frame_channel(&self, task: &Task, fifo_name: &str) -> Option<FrameChannel> {
        let fifo = task.fifos.get(fifo_name)?;
        let view = |end| {
            let end = self.fifo_endpoint(task, fifo_name, end)?;
            self.view_frame(&end.reference.0, end.binding?.port_name)
        };
        let producer = view(fifo.produced_by.as_ref());
        let consumer = view(fifo.consumed_by.as_ref());
        let frame = producer.or(consumer)?;
        Some(FrameChannel {
            frame,
            // A stream the task takes rather than declares has no depth.
            banks: (fifo.depth.unwrap_or(0) / frame).max(2),
            producer_view: producer.is_some(),
            consumer_view: consumer.is_some(),
        })
    }

    /// The top's stream arguments a device-memory ring carries (see
    /// [`Ring`]), in port order.
    #[must_use]
    pub fn rings(&self) -> Vec<Ring> {
        let Some(top) = self.tasks.get(&self.top) else {
            return Vec::new();
        };
        top.ports
            .iter()
            .filter_map(|port| {
                Some(Ring {
                    port: port.name.clone(),
                    input: port.cat == ArgCategory::Istream,
                    frame: self.view_frame(&self.top, &port.name)?,
                    slots: port.ring?,
                })
            })
            .collect()
    }

    /// A stream that carries `[[tapa::frame(N)]]` frames holds at least one
    /// whole frame, on every target: the AI Engine fills a frame before its
    /// reader sees any of it, so a shallower channel on PL or in software
    /// simulation could deadlock a design the AI Engine runs.
    fn validate_frame_depths(&self) -> Result<(), ParseError> {
        let mut memo = BTreeMap::new();
        for (task_name, task) in &self.tasks {
            for (child_name, instances) in &task.tasks {
                let frames = self.port_frames(child_name, &mut memo);
                for instance in instances {
                    for (port_name, arg) in &instance.args {
                        let (Some(&frame), Some(stream)) = (frames.get(port_name), arg.arg.name())
                        else {
                            continue;
                        };
                        let Some(depth) = task.fifos.get(stream).and_then(|fifo| fifo.depth) else {
                            continue;
                        };
                        if depth < frame {
                            return Err(ParseError::FrameDepth {
                                task: task_name.clone(),
                                stream: stream.to_string(),
                                depth,
                                port: format!("{child_name}.{port_name}"),
                                frame,
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The largest frame each stream port of task `name` moves: a leaf's own
    /// `[[tapa::frame(N)]]`, and for an upper task the frames of the child
    /// ports bound to that port.
    fn port_frames(
        &self,
        name: &str,
        memo: &mut BTreeMap<String, BTreeMap<String, u32>>,
    ) -> BTreeMap<String, u32> {
        if let Some(known) = memo.get(name) {
            return known.clone();
        }
        let mut frames = BTreeMap::new();
        if let Some(task) = self.tasks.get(name) {
            for port in &task.ports {
                if let Some(frame) = port.frame {
                    frames.insert(port.name.clone(), frame);
                }
            }
            for (child_name, instances) in &task.tasks {
                let child = self.port_frames(child_name, memo);
                for (port_name, arg) in instances.iter().flat_map(|instance| &instance.args) {
                    if let (Some(&frame), Some(outer)) = (child.get(port_name), arg.arg.name()) {
                        if task.port(outer).is_some() {
                            let entry = frames.entry(outer.to_string()).or_insert(0);
                            *entry = (*entry).max(frame);
                        }
                    }
                }
            }
        }
        memo.insert(name.to_string(), frames.clone());
        frames
    }

    /// Reject invalid invoke-site constants and undeclared or non-scalar child
    /// ports at the boundary, before they render invalid Verilog downstream.
    fn validate_literals(&self) -> Result<(), ParseError> {
        for (task_name, task) in &self.tasks {
            for (child_name, instances) in &task.tasks {
                for instance in instances {
                    for (port_name, arg) in &instance.args {
                        if let crate::instance::ArgSource::Literal(value) = &arg.arg {
                            let path =
                                format!("tasks.{task_name}.tasks.{child_name}.args.{port_name}");
                            if !value.is_valid() {
                                return Err(ParseError::Schema {
                                    path,
                                    message: format!(
                                        "invalid constant {value}: the value must fit \
                                         a width of 1..=64 bits"
                                    ),
                                });
                            }
                            // Only a scalar port can take a constant. Every
                            // stream/mmap consumer resolves args by name and
                            // would silently skip the binding, leaving a
                            // child port unconnected.
                            if arg.cat != crate::port::ArgCategory::Scalar {
                                return Err(ParseError::Schema {
                                    path,
                                    message: format!(
                                        "constant {value} bound to a `{}` port; only \
                                         scalar ports take constants",
                                        arg.cat.as_str()
                                    ),
                                });
                            }
                            let port = self
                                .tasks
                                .get(child_name)
                                .and_then(|child| child.port(port_name))
                                .filter(|port| port.name == *port_name)
                                .ok_or_else(|| ParseError::Schema {
                                    path: path.clone(),
                                    message: format!(
                                        "constant {value} has no declared child port \
                                         `{child_name}.{port_name}`"
                                    ),
                                })?;
                            if !port.cat.is_scalar() {
                                return Err(ParseError::Schema {
                                    path,
                                    message: format!(
                                        "constant {value} bound to a declared `{}` port; \
                                         only scalar ports take constants",
                                        port.cat.as_str()
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_schema_version_is_rejected_as_pre_versioning() {
        // A pre-versioning graph's invoke constants are strings that this
        // schema would silently read as wire names; refuse it like the
        // WorkState v4 bump refuses stale work states.
        let err = TaskGraph::from_json(r#"{"top": "T", "target": "xilinx-hls", "tasks": {}}"#)
            .expect_err("pre-versioning graph must be rejected");
        assert!(
            matches!(err, ParseError::OutdatedSchemaVersion { found: 1, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("regenerate"), "{err}");
    }

    #[test]
    fn invalid_invoke_constant_is_rejected_at_the_boundary() {
        let payload = r#"{
            "schema_version": 4, "top": "T", "target": "xilinx-hls", "cflags": [],
            "tasks": {
                "T": {"level": "upper", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "readable_name": "T", "synth": "hls",
                    "ports": [], "fifos": {},
                    "tasks": {"A": [{"args": {
                        "n": {"arg": {"width": 8, "value": 300}, "cat": "scalar"}
                    }, "step": 0}]}},
                "A": {"level": "lower", "srcs": ["A.cpp"], "include_dirs": [], "defines": [], "readable_name": "A", "synth": "hls",
                    "ports": [{"cat": "scalar", "name": "n", "type": "uint8_t", "width": 8}]}
            }
        }"#;
        let err = TaskGraph::from_json(payload).expect_err("8'd300 must not parse");
        assert!(err.to_string().contains("invalid constant"), "{err}");
        let state = format!(
            r#"{{"version": {}, "graph": {payload}, "flow": {{}}}}"#,
            crate::work_state::VERSION
        );
        let err = crate::WorkState::from_json(&state)
            .expect_err("archiving a graph must not bypass literal validation");
        assert!(err.to_string().contains("invalid constant"), "{err}");
        assert!(err.to_string().contains("graph.tasks.T"), "{err}");
    }

    #[test]
    fn invoke_constant_must_match_declared_scalar_port() {
        let payload = r#"{
            "schema_version": 4, "top": "T", "target": "xilinx-hls",
            "tasks": {
                "T": {"level": "upper", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "readable_name": "T", "synth": "hls",
                    "tasks": {"A": [{"args": {
                        "n": {"arg": {"width": 32, "value": 1}, "cat": "scalar"}
                    }}]}},
                "A": {"level": "lower", "srcs": ["A.cpp"], "include_dirs": [], "defines": [], "readable_name": "A", "synth": "hls",
                    "ports": [{"cat": "declared_category", "name": "n", "type": "int", "width": 32}]}
            }
        }"#;
        let valid = payload.replace("declared_category", "scalar");
        let graph = TaskGraph::from_json(&valid).expect("constant binds to an actual scalar port");
        let state = crate::WorkState::new(graph);
        assert_eq!(
            crate::WorkState::from_json(&serde_json::to_string(&state).expect("serialize state"))
                .expect("archived scalar constant remains valid"),
            state,
            "an actual scalar constant must survive archived-state parsing",
        );

        let mut cases: Vec<_> = ["istream", "ostream", "mmap", "async_mmap"]
            .into_iter()
            .map(|category| (category, payload.replace("declared_category", category)))
            .collect();
        let mut missing_task = state.graph.clone();
        missing_task.tasks.remove("A");
        cases.push((
            "missing child task",
            serde_json::to_string(&missing_task).expect("serialize missing child task"),
        ));
        let mut missing_port = state.graph.clone();
        missing_port
            .tasks
            .get_mut("A")
            .expect("child task")
            .ports
            .clear();
        cases.push((
            "missing child port",
            serde_json::to_string(&missing_port).expect("serialize missing child port"),
        ));
        let mut wrong_port = state.graph.clone();
        wrong_port.tasks.get_mut("A").expect("child task").ports[0].name = "other".into();
        cases.push((
            "wrong child port name",
            serde_json::to_string(&wrong_port).expect("serialize wrong child port name"),
        ));
        for name in ["n[0]", "n[bad]"] {
            let mut indexed_port = state.graph.clone();
            let child = &mut indexed_port
                .tasks
                .get_mut("T")
                .expect("parent task")
                .tasks
                .get_mut("A")
                .expect("child instances")[0];
            let arg = child.args.remove("n").expect("constant binding");
            child.args.insert(name.into(), arg);
            cases.push((
                name,
                serde_json::to_string(&indexed_port).expect("serialize indexed scalar port"),
            ));
        }

        let mut accepted = Vec::new();
        for (case, malformed) in cases {
            match TaskGraph::from_json(&malformed) {
                Ok(_) => accepted.push((case, "graph")),
                Err(error) => assert!(
                    error.to_string().contains("tasks.T.tasks.A.args.n"),
                    "{case}: {error}"
                ),
            }
            let state = format!(
                r#"{{"version": {}, "graph": {malformed}, "flow": {{}}}}"#,
                crate::work_state::VERSION
            );
            match crate::WorkState::from_json(&state) {
                Ok(_) => accepted.push((case, "state")),
                Err(error) => assert!(
                    error.to_string().contains("graph.tasks.T.tasks.A.args.n"),
                    "{case}: {error}"
                ),
            }
        }
        assert!(
            accepted.is_empty(),
            "constants must require declared scalar ports; accepted: {accepted:?}"
        );
    }

    #[test]
    fn current_schema_version_is_accepted() {
        let g = TaskGraph::from_json(
            r#"{"schema_version": 4, "top": "T", "target": "xilinx-hls", "tasks": {}}"#,
        )
        .unwrap();
        assert_eq!(g.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn newer_schema_version_is_rejected_with_regenerate_message() {
        let err = TaskGraph::from_json(
            r#"{"schema_version": 999, "top": "T", "target": "xilinx-hls", "tasks": {}}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("999") && msg.contains("regenerate"), "{msg}");
    }

    #[test]
    fn parses_named_child_instances() {
        TaskGraph::from_json(
            r#"{
              "schema_version": 4,
        "cflags": [],
              "top": "Top",
              "target": "xilinx-hls",
              "tasks": {
                "Leaf": {
                    "readable_name": "Leaf",
                  "srcs": ["Leaf.cpp"], "include_dirs": [], "defines": [],
                  "level": "lower",
                  "synth": "hls",
                  "ports": [],
                  "tasks": {},
                  "fifos": {}
                },
                "Top": {
                    "readable_name": "Top",
                  "srcs": ["Top.cpp"], "include_dirs": [], "defines": [],
                  "level": "upper",
                  "synth": "hls",
                  "ports": [],
                  "tasks": {
                    "Leaf": [
                      {"name": "Leaf_0", "args": {}, "step": 0}
                    ]
                  },
                  "fifos": {}
                }
              }
            }"#,
        )
        .expect("graph parses");
    }

    #[test]
    fn missing_root_target_rejected() {
        // The flow target is required at the root now that it is the single
        // home for the vendor flow (analyze injects it before parsing).
        let err = TaskGraph::from_json(
            r#"{"schema_version":4,"cflags":[],"top":"T","tasks":{"T":{"srcs":["T.cpp"],"include_dirs":[],"defines":[],"level":"upper","synth":"hls","readable_name":"T","ports":[],"tasks":{},"fifos":{}}}}"#,
        )
        .expect_err("missing root `target` must fail");
        assert!(
            err.to_string().contains("target"),
            "error must point at the missing target field; got {err}",
        );
    }

    #[test]
    fn a_stream_must_hold_a_whole_frame_through_upper_tasks() {
        // Leaf's frame reaches T's stream `q` through Mid's port `out`.
        let graph = |depth: u32| {
            let task = |level: &str, ports: &str, tasks: &str| {
                format!(
                    r#"{{"level": "{level}", "srcs": [], "include_dirs": [], "defines": [],
                        "synth": "hls", "readable_name": "", "ports": [{ports}],
                        "tasks": {{{tasks}}}, "fifos": {{}}}}"#
                )
            };
            let out = r#"{"cat": "ostream", "name": "out", "type": "int", "width": 32}"#;
            let framed =
                r#"{"cat": "ostream", "name": "out", "type": "int", "width": 32, "frame": 8}"#;
            let top = task(
                "upper",
                "",
                r#""Mid": [{"step": 0, "args": {"out": {"arg": "q", "cat": "ostream"}}}],
                   "Sink": [{"step": 0, "args": {"in": {"arg": "q", "cat": "istream"}}}]"#,
            )
            .replace(
                r#""fifos": {}"#,
                &format!(
                    r#""fifos": {{"q": {{"depth": {depth}, "produced_by": ["Mid", 0],
                                         "consumed_by": ["Sink", 0]}}}}"#
                ),
            );
            let mid = task(
                "upper",
                out,
                r#""Leaf": [{"step": 0, "args": {"out": {"arg": "out", "cat": "ostream"}}}]"#,
            );
            let leaf = task("lower", framed, "");
            let sink = task(
                "lower",
                r#"{"cat": "istream", "name": "in", "type": "int", "width": 32}"#,
                "",
            );
            format!(
                r#"{{"schema_version": {SCHEMA_VERSION}, "top": "T", "target": "xilinx-hls",
                    "cflags": [], "tasks": {{"T": {top}, "Mid": {mid}, "Leaf": {leaf},
                    "Sink": {sink}}}}}"#
            )
        };
        let error = TaskGraph::from_json(&graph(4)).expect_err("a frame of 8 in depth 4");
        let message = error.to_string();
        assert!(
            message.contains("stream `q` of task `T` has depth 4"),
            "{message}"
        );
        assert!(message.contains("`Mid.out` moves frames of 8"), "{message}");
        TaskGraph::from_json(&graph(8)).expect("depth 8 holds a frame of 8");
    }

    #[test]
    fn a_frame_port_reaches_a_frame_channel_whose_ends_agree_on_the_frame() {
        // A port spec is `cat` or `cat:frame`.
        let port = |name: &str, spec: &str| {
            let (cat, frame) = spec.split_once(':').unwrap_or((spec, ""));
            let frame = if frame.is_empty() {
                String::new()
            } else {
                format!(r#", "frame": {frame}"#)
            };
            format!(r#"{{"cat": "{cat}", "name": "{name}", "type": "int", "width": 32{frame}}}"#)
        };
        let bind = |child: &str, port: &str, arg: &str, spec: &str| {
            let cat = spec.split(':').next().unwrap_or(spec);
            format!(
                r#""{child}": [{{"step": 0, "args": {{"{port}": {{"arg": "{arg}", "cat": "{cat}"}}}}}}]"#
            )
        };
        let task = |level: &str, synth: &str, ports: &str, tasks: &str, fifos: &str| {
            format!(
                r#"{{"level": "{level}", "srcs": [], "include_dirs": [], "defines": [],
                    "synth": "{synth}", "readable_name": "", "ports": [{ports}],
                    "tasks": {{{tasks}}}, "fifos": {{{fifos}}}}}"#
            )
        };
        let graph = |tasks: &[(&str, String)]| {
            let tasks: Vec<String> = tasks
                .iter()
                .map(|(n, t)| format!(r#""{n}": {t}"#))
                .collect();
            format!(
                r#"{{"schema_version": {SCHEMA_VERSION}, "top": "T", "target": "xilinx-hls",
                    "cflags": [], "tasks": {{{}}}}}"#,
                tasks.join(", ")
            )
        };
        // T streams `q` from `Src.out` to `Dst.in`.
        let pair = |out: &str, input: &str, dst_synth: &str| {
            let q = r#""q": {"depth": 16, "produced_by": ["Src", 0], "consumed_by": ["Dst", 0]}"#;
            let tasks = format!(
                "{}, {}",
                bind("Src", "out", "q", out),
                bind("Dst", "in", "q", input)
            );
            graph(&[
                ("T", task("upper", "hls", "", &tasks, q)),
                ("Src", task("lower", "hls", &port("out", out), "", "")),
                ("Dst", task("lower", dst_synth, &port("in", input), "", "")),
            ])
        };
        let rejects = |json: String, needle: &str| {
            let message = TaskGraph::from_json(&json)
                .expect_err("rejected")
                .to_string();
            assert!(message.contains(needle), "{message}");
        };
        TaskGraph::from_json(&pair("oframes:8", "istream", "hls")).expect("view to stream");
        TaskGraph::from_json(&pair("ostream", "iframes:8", "aie")).expect("PLIO into an AIE view");
        rejects(
            pair("oframes:8", "iframes:4", "hls"),
            "stream `q` of task `T` cannot be a frame channel: its producer moves frames of 8 \
             elements and its consumer frames of 4",
        );
        // A PL frame port at a PLIO gets its channel in the PL kernel's top.
        TaskGraph::from_json(&pair("oframes:8", "istream", "aie")).expect("PL view into a PLIO");

        // `Dst` is an upper task handing its own port `in` to `View`'s frame
        // port `in`, or `z`, which it neither declares nor takes.
        let through = |arg: &str| {
            let q = r#""q": {"depth": 16, "produced_by": ["Src", 0], "consumed_by": ["Dst", 0]}"#;
            let tasks = format!(
                "{}, {}",
                bind("Src", "out", "q", "ostream"),
                bind("Dst", "in", "q", "istream")
            );
            graph(&[
                ("T", task("upper", "hls", "", &tasks, q)),
                ("Src", task("lower", "hls", &port("out", "ostream"), "", "")),
                (
                    "Dst",
                    task(
                        "upper",
                        "hls",
                        &port("in", "istream"),
                        &bind("View", "in", arg, "iframes"),
                        "",
                    ),
                ),
                (
                    "View",
                    task("lower", "hls", &port("in", "iframes:8"), "", ""),
                ),
            ])
        };
        let g = TaskGraph::from_json(&through("in")).expect("a view through an upper task");
        assert_eq!(g.view_frame("Dst", "in"), Some(8));
        assert_eq!(g.view_frame("Src", "out"), None);
        rejects(
            through("z"),
            "stream `z` of task `Dst` cannot be a frame channel: frame port `in` must bind a \
             stream the task declares or one of its stream ports",
        );

        // A leaf top has no task to hold a frame channel.
        rejects(
            graph(&[("T", task("lower", "hls", &port("in", "iframes:8"), "", ""))]),
            "a top task's frame port has no task to hold its frame channel",
        );
    }
}
