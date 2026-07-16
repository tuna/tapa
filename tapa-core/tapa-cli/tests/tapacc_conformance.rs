//! Producer↔consumer schema conformance: the **real** `tapacc` binary's
//! stdout must parse under `tapa-ir`'s strict (`deny_unknown_fields`) types.
//!
//! Every other test of the graph schema parses a *hand-written* fixture, so
//! the corpus structurally cannot notice the C++ producer drifting away from
//! the Rust consumer: both sides of a hand-written fixture are written by
//! whoever changed the Rust. That drift has already happened once — `tapacc`
//! emitted a per-task `"target": "xilinx_vitis"` while `tapa-ir` required
//! `"synth": "hls"` — and only a manual Linux run caught it. This test is the
//! guard for that seam.
//!
//! # What it actually runs
//!
//! `tapacc` is a Clang tool: driving it needs a flattened translation unit
//! (from `tapa-cpp`), a `-resource-dir` pointing at Clang's staged builtin
//! headers, and the TAPA/vendor include cascade. Hand-rolling that argv in a
//! test would create a *second* invocation that can itself drift from the
//! real one — a fake front-end wearing tapacc's name. So the test drives the
//! production invocation instead: `tapa analyze`, which already composes
//! `tapa-cpp` + `tapacc` and drops `tapacc`'s stdout **verbatim** at
//! `<work_dir>/tapacc.json` (see `steps::analyze::TAPACC_ARTIFACT`). Those
//! bytes are the raw producer output, untouched by analyze's `cflags`/`target`
//! injection, and they are what gets strict-parsed below.
//!
//! # Wiring
//!
//! The inputs arrive as runfiles-relative paths in the environment, set by
//! `//tapa-core:tapacc_conformance_test` (`tapa-core/BUILD.bazel`). A plain
//! `cargo test` sets none of them and the test skips; `TAPA_CONFORMANCE_REQUIRED`
//! is what stops that skip from silently swallowing the guard in CI.
//!
//!     bazel test //tapa-core:tapacc_conformance_test    # Linux only

use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tapa_ir::{TaskGraph, TaskLevel};

/// Runfiles-relative path of the `tapa` CLI wrapper (`//tapa-core:tapa`),
/// whose runfiles stage the sibling `tapacc` / `tapa-cpp` / system headers.
const ENV_TAPA: &str = "TAPA_CONFORMANCE_TAPA";
/// Runfiles-relative path of the `tapa-lib` include directory.
const ENV_TAPA_LIB: &str = "TAPA_CONFORMANCE_TAPA_LIB";
/// Set by the Bazel target only. Turns "inputs missing" from a skip into a
/// failure, so a broken `cargo_env` block cannot quietly disable the guard.
const ENV_REQUIRED: &str = "TAPA_CONFORMANCE_REQUIRED";

/// One kernel in the conformance corpus.
struct Kernel {
    /// Env var holding the runfiles-relative path to the `.cpp` file.
    env: &'static str,
    /// Top-level task name.
    top: &'static str,
    /// Tasks `tapacc` must report.
    expected_tasks: &'static [&'static str],
    /// Which flows to test (vadd tests both; the rest test hls only).
    flows: &'static [&'static str],
}

/// The full corpus. vadd is the original; the rest close coverage gaps:
/// `async_mmap`, template-specialization (`readable_name != name`), and
/// `synth: "ignore"` (custom-RTL policy).
const KERNELS: &[Kernel] = &[
    Kernel {
        env: "TAPA_CONFORMANCE_VADD",
        top: "VecAdd",
        expected_tasks: &["Add", "Mmap2Stream", "Stream2Mmap", "VecAdd"],
        flows: &["xilinx-hls", "xilinx-vitis"],
    },
    Kernel {
        env: "TAPA_CONFORMANCE_ASYNC_MMAP",
        top: "AsyncTop",
        expected_tasks: &["AsyncReader", "AsyncTop"],
        flows: &["xilinx-hls"],
    },
    Kernel {
        env: "TAPA_CONFORMANCE_TEMPLATED",
        top: "TemplatedTop",
        expected_tasks: &["Mmap2Stream", "Stream2Mmap", "TemplatedTop"],
        flows: &["xilinx-hls"],
    },
    Kernel {
        env: "TAPA_CONFORMANCE_IGNORE",
        top: "IgnoreTop",
        // `Add` is NOT here: tapcc does not descend into `synth: "ignore"`
        // tasks (their children are user-provided custom RTL).
        expected_tasks: &["IgnoreUpper", "Mmap2Stream", "Stream2Mmap", "IgnoreTop"],
        flows: &["xilinx-hls"],
    },
];

/// Resolved test inputs.
struct Fixtures {
    tapa: PathBuf,
    tapa_lib: PathBuf,
    /// Maps env-var name → resolved kernel `.cpp` path.
    kernels: std::collections::BTreeMap<&'static str, PathBuf>,
}

/// Resolve a runfiles-relative path against the Bazel runfiles tree.
///
/// Mirrors `tools/test-tools`'s `common::workspace_path`, which is the
/// resolution this repo's other Bazel-driven Rust tools already rely on.
fn resolve_runfile(rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches("_main/");
    let direct = Path::new(rel);
    if direct.exists() {
        return direct.canonicalize().ok();
    }
    let workspace = env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
    for base_var in ["RUNFILES_DIR", "TEST_SRCDIR"] {
        let Some(base) = env::var_os(base_var) else {
            continue;
        };
        let base = Path::new(&base);
        for candidate in [
            base.join(&workspace).join(rel),
            base.join("_main").join(rel),
            base.join(rel),
        ] {
            if candidate.exists() {
                return candidate.canonicalize().ok();
            }
        }
    }
    None
}

/// Read one required runfiles-path variable. Only called once [`ENV_TAPA`] is
/// known to be set, so a missing sibling is a wiring bug, not a skip.
fn runfile_var(var: &str) -> PathBuf {
    let value =
        env::var(var).unwrap_or_else(|e| panic!("{var} must be set alongside {ENV_TAPA} ({e})"));
    resolve_runfile(&value)
        .unwrap_or_else(|| panic!("{var}={value} does not resolve to an existing runfile"))
}

/// Resolve the inputs, or `None` when this is a plain `cargo test` run.
fn fixtures() -> Option<Fixtures> {
    let Some(tapa) = env::var_os(ENV_TAPA) else {
        assert!(
            env::var_os(ENV_REQUIRED).is_none(),
            "{ENV_REQUIRED} is set but {ENV_TAPA} is not: the Bazel target's \
             `cargo_env` block has drifted from this test. A conformance guard \
             that skips itself in CI is worse than no guard.",
        );
        eprintln!(
            "tapacc conformance: {ENV_TAPA} unset; skipping. This test runs \
             under `bazel test //tapa-core:tapacc_conformance_test` (Linux \
             only), which builds the real //tapacc binary.",
        );
        return None;
    };
    let tapa = tapa
        .to_str()
        .unwrap_or_else(|| panic!("{ENV_TAPA} must be UTF-8"))
        .to_string();
    let tapa_resolved = resolve_runfile(&tapa)
        .unwrap_or_else(|| panic!("{ENV_TAPA}={tapa} does not resolve to an existing runfile"));
    let tapa_lib = runfile_var(ENV_TAPA_LIB);
    let mut kernels = std::collections::BTreeMap::new();
    for k in KERNELS {
        kernels.insert(k.env, runfile_var(k.env));
    }
    Some(Fixtures {
        tapa: tapa_resolved,
        tapa_lib,
        kernels,
    })
}

/// Run `tapa analyze` for `flow` and return `tapacc`'s verbatim stdout.
fn tapacc_stdout(fx: &Fixtures, kernel: &Path, top: &str, flow: &str) -> String {
    let work = tempfile::Builder::new()
        .prefix(&format!("tapacc-conformance-{flow}-"))
        .tempdir()
        .expect("create work dir");
    let kernel_dir = kernel.parent().expect("kernel .cpp has a parent directory");

    let output = Command::new(&fx.tapa)
        .arg("--work-dir")
        .arg(work.path())
        .arg("analyze")
        .arg("--input")
        .arg(kernel)
        .arg("--top")
        .arg(top)
        .arg("--target")
        .arg(flow)
        .arg("--cflags")
        .arg(format!("-I{}", kernel_dir.display()))
        .arg("--cflags")
        .arg(format!("-I{}", fx.tapa_lib.display()))
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", fx.tapa.display()));

    assert!(
        output.status.success(),
        "`tapa analyze --target {flow}` failed for {} ({})\nstdout:\n{}\nstderr:\n{}",
        kernel.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // `analyze` writes this before it interprets anything, so these are
    // tapacc's own bytes: no injected `cflags`, no injected `target`.
    let raw_path = work.path().join("tapacc.json");
    std::fs::read_to_string(&raw_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", raw_path.display()))
}

/// Strict-parse `raw` with the consumer's real types and assert the
/// load-bearing facts of the contract.
fn check_conformance(raw: &str, flow: &str, top: &str, expected_tasks: &[&str]) {
    // ── The guard ──────────────────────────────────────────────────────
    // `TaskGraph` and every type under it are `deny_unknown_fields`, so this
    // fails if tapacc grew a field tapa-ir does not model, and fails if it
    // stopped emitting a required one. tapacc emits `{top, target, tasks}`;
    // `cflags` is the one root key it leaves to `analyze`, and tapa-ir
    // defaults it.
    let graph = TaskGraph::from_json(raw).unwrap_or_else(|e| {
        panic!(
            "tapacc's stdout does not conform to the tapa-ir schema \
             (--target {flow}): {e}\n\
             \n\
             This is producer/consumer drift. Either `tapacc/tapacc.cpp` \
             emits a field `tapa-ir` does not model, or it stopped emitting a \
             required one. Fix whichever side is wrong — widening the schema \
             to make this pass is only correct if the new wire form is the \
             one you meant.\n\
             \n\
             raw tapacc output:\n{raw}",
        )
    });

    assert_eq!(graph.top, top, "root `top` names the analyzed top task");
    assert_eq!(
        graph.target.as_str(),
        flow,
        "root `target` must be the kebab-case flow that was requested",
    );

    let names: BTreeSet<&str> = graph.tasks.keys().map(String::as_str).collect();
    for expected in expected_tasks {
        assert!(
            names.contains(expected),
            "task `{expected}` missing from tapacc output (--target {flow}); got {names:?}",
        );
    }
    assert_eq!(
        graph.tasks[top].level,
        TaskLevel::Upper,
        "the top task of {top} is an upper task",
    );
    for (name, task) in &graph.tasks {
        assert!(
            !task.code.is_empty(),
            "task `{name}`: tapacc emitted empty `code` (--target {flow})",
        );
    }

    // ── Wire-level checks ──────────────────────────────────────────────
    // Deliberately re-read the values as raw JSON rather than through the
    // parsed enums. `Target` and `SynthTarget` are closed today, so the parse
    // above already rejects the bad spellings — but if either enum is ever
    // widened, the typed parse alone would stop catching a producer that
    // emits the old form. These assertions pin the wire strings themselves.
    let json: Value = serde_json::from_str(raw).expect("tapacc emits JSON");
    assert_eq!(
        json["target"].as_str(),
        Some(flow),
        "root `target` on the wire must be the kebab-case flow",
    );
    let tasks = json["tasks"]
        .as_object()
        .expect("tapacc emits an object for `tasks`");
    for (name, task) in tasks {
        let synth = task["synth"]
            .as_str()
            .unwrap_or_else(|| panic!("task `{name}`: `synth` must be a string (--target {flow})"));
        assert!(
            matches!(synth, "hls" | "ignore"),
            "task `{name}`: `synth` is `{synth}` (--target {flow}); the wire \
             contract is `hls`/`ignore` only. The per-task flow spelling \
             (`xilinx_vitis`) is the exact drift this guard exists to catch: \
             the flow belongs at the graph root, and `synth` only answers \
             \"synthesize or skip\".",
        );
        let readable_name = task["readable_name"].as_str().unwrap_or_else(|| {
            panic!("task `{name}`: `readable_name` must be a string (--target {flow})")
        });
        assert!(
            !readable_name.is_empty(),
            "task `{name}`: `readable_name` is empty (--target {flow}); tapacc \
             emits it unconditionally, equal to the task name for \
             non-template tasks",
        );
    }
}

#[test]
fn tapacc_stdout_conforms_to_tapa_ir_schema() {
    let Some(fx) = fixtures() else {
        return;
    };
    for kernel in KERNELS {
        let cpp = &fx.kernels[kernel.env];
        for flow in kernel.flows {
            let raw = tapacc_stdout(&fx, cpp, kernel.top, flow);
            check_conformance(&raw, flow, kernel.top, kernel.expected_tasks);
        }
    }
}
