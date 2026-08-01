//! Private fixture vocabulary shared by the `tapa-codegen` integration
//! tests: parse one Verilog module and assemble a `tapa_ir::Design` from
//! the wire-schema JSON with repeated defaults filled in, so each test
//! only states what varies.
//!
//! Each consumer declares `mod common;` at its target root; files under
//! `tests/common/` do not become integration targets of their own.

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_ir::Design;
use tapa_rtl::VerilogModule;

/// Helper: parse a minimal Verilog module source.
pub fn parse_module(src: &str) -> VerilogModule {
    VerilogModule::parse(src).expect("valid Verilog")
}

// ── Design-fixture builder ──────────────────────────────────────────
//
// Every test needs a `Design` built from the tapa-ir wire schema. The
// raw `json!` literals repeat the same defaults (`readable_name`, `level`,
// `code: ""`, `synth: "hls"`, `ports: []`, `tasks: {}`, `fifos: {}`) for
// every task. These helpers fill the defaults so tests only state what
// varies. `design` assembles the root envelope; `plain`/`task` build one
// task; `attach_basic_modules` attaches the standard ap_clk/ap_rst_n
// module to a set of tasks.

/// Build a `Design` from `(name, task_json)` pairs, wrapped in the
/// standard `{"top", "target", "tasks"}` envelope.
pub fn design(top: &str, target: &str, tasks: &[(&str, serde_json::Value)]) -> Design {
    let tasks: serde_json::Value = tasks
        .iter()
        .map(|(name, t)| (name.to_string(), t.clone()))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    serde_json::from_value(serde_json::json!({
        "top": top,
        "target": target,
        "tasks": tasks,
    }))
    .expect("valid design fixture JSON")
}

/// A task with all defaults (no ports, children, or fifos).
pub fn plain(name: &str, level: &str) -> serde_json::Value {
    task(name, level, |_| {})
}

/// A task with defaults, plus overrides applied by `f` to the JSON object.
/// `f` receives the task JSON and may set `ports` / `tasks` / `fifos`.
pub fn task(name: &str, level: &str, f: impl FnOnce(&mut serde_json::Value)) -> serde_json::Value {
    let mut t = serde_json::json!({
        "readable_name": name,
        "level": level,
        "code": "",
        "synth": "hls",
        "ports": [],
        "tasks": {},
        "fifos": {},
    });
    f(&mut t);
    t
}

/// The standard minimal module source (`ap_clk` + `ap_rst_n` only).
fn basic_module_src(name: &str) -> String {
    format!("module {name}(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule")
}

/// Attach `basic_module_src` to each task in `names`.
pub fn attach_basic_modules(state: &mut TopologyWithRtl, names: &[&str]) {
    for name in names {
        state
            .attach_module(name, parse_module(&basic_module_src(name)))
            .unwrap();
    }
}
