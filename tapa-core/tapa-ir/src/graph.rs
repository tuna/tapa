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
use crate::target::Target;
use crate::task::Task;

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
        self.validate_frame_depths()
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
}
