//! Private fixture vocabulary shared by the `tapa-codegen` integration
//! tests: parse one Verilog module and assemble a `tapa_ir::Design` from
//! the wire-schema JSON with repeated defaults filled in, so each test
//! only states what varies. [`run_manifest`] drives the full
//! `generate_rtl` pipeline for the behavior tests and hands back the
//! shipped `ArtifactManifest`.
//!
//! Each consumer declares `mod common;` at its target root; files under
//! `tests/common/` do not become integration targets of their own.

#![allow(
    dead_code,
    reason = "every consumer compiles the whole shared vocabulary but \
              asserts against only its own slice; the rest would read as \
              dead code in that target"
)]

pub mod run_manifest;
pub mod verilator;

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
        .map(|(name, t)| ((*name).to_string(), t.clone()))
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

// ── Shared module-source fixtures ───────────────────────────────────
//
// Module sources shared by two or more tests, parameterized only by the
// module name. Single-use sources stay inline at their test.

/// The standard minimal module source (`ap_clk` + `ap_rst_n` only).
fn basic_module_src(name: &str) -> String {
    format!("module {name}(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule")
}

/// The full `ap_ctrl` handshake shell (`ap_clk`, `ap_rst_n`, `ap_start`,
/// `ap_done`, `ap_idle`, `ap_ready`) with no user ports.
pub fn handshake_module_src(name: &str) -> String {
    format!(
        "module {name}(\n\
         input wire ap_clk,\n\
         input wire ap_rst_n,\n\
         input wire ap_start,\n\
         output wire ap_done,\n\
         output wire ap_idle,\n\
         output wire ap_ready\n\
         );\nendmodule"
    )
}

/// A child with 1-bit AXI IDs on its `mmap` port
/// (`m_axi_mmap_{AR,AW,B,R}ID`): the narrow side of a parent-crossbar ID
/// width test.
pub fn narrow_axi_id_module_src(name: &str) -> String {
    format!(
        "module {name}(\n\
         input wire ap_clk,\n\
         input wire ap_rst_n,\n\
         output wire m_axi_mmap_ARID,\n\
         output wire m_axi_mmap_AWID,\n\
         input wire m_axi_mmap_BID,\n\
         input wire m_axi_mmap_RID\n\
         ); endmodule"
    )
}

/// A child with 2-bit AXI IDs on its `data` port
/// (`m_axi_data_{AR,AW,B,R}ID`): the wide side of a parent-crossbar ID
/// width test.
pub fn wide_axi_id_module_src(name: &str) -> String {
    format!(
        "module {name}(\n\
         input wire ap_clk,\n\
         input wire ap_rst_n,\n\
         output wire [1:0] m_axi_data_ARID,\n\
         output wire [1:0] m_axi_data_AWID,\n\
         input wire [1:0] m_axi_data_BID,\n\
         input wire [1:0] m_axi_data_RID\n\
         ); endmodule"
    )
}

/// A 32-bit streaming producer child (`out_data`), bound to `fifo_0`'s
/// producing side in the shared producer/consumer fixtures.
pub fn stream_producer_module_src(name: &str) -> String {
    format!(
        "module {name}(\n\
         input wire ap_clk,\n\
         input wire ap_rst_n,\n\
         output wire [31:0] out_data_din,\n\
         output wire out_data_write,\n\
         input wire out_data_full_n\n\
         );\nendmodule"
    )
}

/// The matching 32-bit streaming consumer child (`in_data`).
pub fn stream_consumer_module_src(name: &str) -> String {
    format!(
        "module {name}(\n\
         input wire ap_clk,\n\
         input wire ap_rst_n,\n\
         input wire [31:0] in_data_dout,\n\
         input wire in_data_empty_n,\n\
         output wire in_data_read\n\
         );\nendmodule"
    )
}

/// Attach `basic_module_src` to each task in `names`.
pub fn attach_basic_modules(state: &mut TopologyWithRtl, names: &[&str]) {
    for name in names {
        state
            .attach_module(name, parse_module(&basic_module_src(name)))
            .unwrap();
    }
}
