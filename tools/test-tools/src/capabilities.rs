//! `mf-capabilities`: end-to-end contract test for the multi-file
//! frontend. `tests/apps/multi-file/testdata/capabilities.json` names one
//! probe per entry; every probe drives `tapa analyze` (the multi-file
//! app, plus a vadd positive control) and must pass, so a regression
//! fails the test with the manifest naming the contract that broke.

use serde::Deserialize;
use serde_json::{Map, Value as JsonValue};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

use crate::common::{read_json, require_file, workspace_path, Result};

/// Manifest location, relative to the workspace root.
const MANIFEST: &str = "tests/apps/multi-file/testdata/capabilities.json";
/// Positive-control app analyzed by `sanity_vadd_analyze`.
const VADD_SOURCE: &str = "tests/apps/vadd/vadd.cpp";
/// The multi-file app under test.
const APP_A: &str = "tests/apps/multi-file/a.cpp";
const APP_B: &str = "tests/apps/multi-file/b.cpp";
/// Include dir outside the app dir, reached via `-I`.
const APP_EXT_INCLUDE: &str = "tests/apps/multi-file-ext";

#[derive(Deserialize)]
struct Manifest {
    capabilities: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    /// One line: the contract this entry pins.
    note: String,
}

/// What the current pipeline actually did with a capability.
enum ProbeOutcome {
    Pass,
    Fail(String),
}

/// Shared probe context: the tools to drive.
struct Env<'a> {
    tapa: &'a Path,
    tapa_lib: &'a Path,
}

impl Env<'_> {
    /// The flag putting `tapa_lib` on the compiler's include path. It rides
    /// `-isystem`, the same class of entry production `tapa analyze` adds
    /// through `get_tapacc_cflags`: tapa-lib is vendor support, not user
    /// code the mirror owns.
    fn tapa_lib_flag(&self) -> String {
        format!("-isystem{}", self.tapa_lib.display())
    }
}

/// A named probe: runs one capability check against the current
/// pipeline and reports what actually happened.
type Probe = fn(&Env) -> Result<ProbeOutcome>;

const PROBES: &[(&str, Probe)] = &[
    ("sanity_vadd_analyze", probe_sanity_vadd_analyze),
    ("multi_tu_analyze", probe_multi_tu_analyze),
    ("macro_invoke_seen", probe_macro_invoke_seen),
    (
        "synthesis_branch_preserved",
        probe_synthesis_branch_preserved,
    ),
    ("header_not_inlined", probe_header_not_inlined),
    ("line_directive_present", probe_line_directive_present),
];

pub fn mf_capabilities() -> Result<()> {
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(workspace_path(MANIFEST))
            .map_err(|error| format!("failed to read {MANIFEST}: {error}"))?,
    )
    .map_err(|error| format!("invalid manifest {MANIFEST}: {error}"))?;

    let tapa = workspace_path("tapa-core/tapa");
    let tapa_lib = workspace_path("tapa-lib");
    let mut rows = Vec::new();
    for entry in &manifest.capabilities {
        let probe = PROBES
            .iter()
            .find(|(name, _)| *name == entry.name)
            .map(|(_, probe)| *probe)
            .ok_or_else(|| format!("unknown capability '{}' in {MANIFEST}", entry.name))?;
        let env = Env {
            tapa: &tapa,
            tapa_lib: &tapa_lib,
        };
        let actual = probe(&env)?;
        rows.push((entry, actual));
    }

    print_table(&rows);

    let mut failures = Vec::new();
    for (entry, actual) in &rows {
        if let ProbeOutcome::Fail(reason) = actual {
            failures.push(format!("  {}: {reason}", entry.name));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!(
        "capability contracts failed:\n{}\nsee the notes in {MANIFEST}",
        failures.join("\n"),
    ))
}

fn print_table(rows: &[(&Entry, ProbeOutcome)]) {
    let mut table = String::new();
    writeln!(table, "{:<27} {:<7} note", "capability", "actual")
        .expect("write to String cannot fail");
    for (entry, actual) in rows {
        let actual = match actual {
            ProbeOutcome::Pass => "pass".to_string(),
            ProbeOutcome::Fail(reason) => format!("fail ({reason})"),
        };
        writeln!(table, "{:<27} {:<7} {}", entry.name, actual, entry.note)
            .expect("write to String cannot fail");
    }
    println!("{table}");
}

/// `sanity_vadd_analyze` (positive control): single-TU analyze of
/// tests/apps/vadd succeeds and yields exactly the four vadd tasks;
/// proves the harness can detect a pass.
fn probe_sanity_vadd_analyze(env: &Env) -> Result<ProbeOutcome> {
    let source = workspace_path(VADD_SOURCE);
    let source_dir = source
        .parent()
        .ok_or_else(|| format!("source has no parent: {}", source.display()))?;
    let cflags = [format!("-I{}", source_dir.display()), env.tapa_lib_flag()];
    match tapa_analyze(env, "VecAdd", &[source], &cflags)? {
        AnalyzeOutcome::Failure(reason) => Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => {
            let graph = read_graph(work_dir.path())?;
            let mut actual = task_names(&graph)?;
            actual.sort_unstable();
            let mut expected = ["VecAdd", "Mmap2Stream", "Add", "Stream2Mmap"];
            expected.sort_unstable();
            if actual == expected {
                Ok(ProbeOutcome::Pass)
            } else {
                Ok(ProbeOutcome::Fail(format!(
                    "tasks are {actual:?}, expected {expected:?}"
                )))
            }
        }
    }
}

/// `multi_tu_analyze`: the app's two-TU analyze succeeds and the graph
/// contains `MultiFileTop`, `Produce`, `Consume`, and a task whose
/// `readable_name` is "Combine<float>".
fn probe_multi_tu_analyze(env: &Env) -> Result<ProbeOutcome> {
    let work_dir = match analyze_multi_file(env)? {
        AnalyzeOutcome::Failure(reason) => return Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => work_dir,
    };
    let graph = read_graph(work_dir.path())?;
    let tasks = tasks_object(&graph)?;
    for name in ["MultiFileTop", "Produce", "Consume"] {
        if !tasks.contains_key(name) {
            let found: Vec<&String> = tasks.keys().collect();
            return Ok(ProbeOutcome::Fail(format!(
                "task '{name}' missing from the graph; found {found:?}"
            )));
        }
    }
    let readable: Vec<&str> = tasks
        .values()
        .filter_map(|task| task.get("readable_name").and_then(JsonValue::as_str))
        .collect();
    if readable.contains(&"Combine<float>") {
        Ok(ProbeOutcome::Pass)
    } else {
        Ok(ProbeOutcome::Fail(format!(
            "no task with readable_name 'Combine<float>'; readable names: {readable:?}"
        )))
    }
}

/// `macro_invoke_seen`: two-TU analyze succeeds AND `MultiFileTop`'s
/// instances include `Consume` (the invoke wrapped in the
/// `INVOKE_CONSUME` macro is in the graph).
fn probe_macro_invoke_seen(env: &Env) -> Result<ProbeOutcome> {
    let work_dir = match analyze_multi_file(env)? {
        AnalyzeOutcome::Failure(reason) => return Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => work_dir,
    };
    let graph = read_graph(work_dir.path())?;
    let top = tasks_object(&graph)?
        .get("MultiFileTop")
        .ok_or_else(|| "graph has no task 'MultiFileTop'".to_string())?;
    let instances = top
        .get("tasks")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "MultiFileTop missing object 'tasks' (instances)".to_string())?;
    match instances.get("Consume").and_then(JsonValue::as_array) {
        Some(found) if !found.is_empty() => Ok(ProbeOutcome::Pass),
        _ => {
            let found: Vec<&String> = instances.keys().collect();
            Ok(ProbeOutcome::Fail(format!(
                "MultiFileTop does not instantiate Consume; instances: {found:?}"
            )))
        }
    }
}

/// `synthesis_branch_preserved`: two-TU analyze succeeds AND `Produce`'s
/// per-task HLS text contains BOTH `kLanes = 4` and `kLanes = 1` (the
/// `#ifdef __SYNTHESIS__` arms survive for the downstream consumer).
fn probe_synthesis_branch_preserved(env: &Env) -> Result<ProbeOutcome> {
    let work_dir = match analyze_multi_file(env)? {
        AnalyzeOutcome::Failure(reason) => return Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => work_dir,
    };
    let texts = per_task_hls_texts(work_dir.path())?;
    let Some(code) = texts
        .iter()
        .find(|(name, _)| name == "Produce")
        .map(|(_, code)| code)
    else {
        return Ok(ProbeOutcome::Fail("no task named 'Produce'".to_string()));
    };
    let mut missing = Vec::new();
    if !code.contains("kLanes = 4") {
        missing.push("`kLanes = 4`");
    }
    if !code.contains("kLanes = 1") {
        missing.push("`kLanes = 1`");
    }
    if missing.is_empty() {
        Ok(ProbeOutcome::Pass)
    } else {
        Ok(ProbeOutcome::Fail(format!(
            "Produce's HLS text lacks {} (an #ifdef arm was baked in)",
            missing.join(" and ")
        )))
    }
}

/// `header_not_inlined`: two-TU analyze succeeds AND no task's HLS text
/// contains `a_q.read() + b_q.read()` (the template task body from
/// multi-file.h must live in a header, not be inlined into every task's
/// source).
fn probe_header_not_inlined(env: &Env) -> Result<ProbeOutcome> {
    let work_dir = match analyze_multi_file(env)? {
        AnalyzeOutcome::Failure(reason) => return Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => work_dir,
    };
    let texts = per_task_hls_texts(work_dir.path())?;
    match texts
        .iter()
        .find(|(_, code)| code.contains("a_q.read() + b_q.read()"))
    {
        Some((name, _)) => Ok(ProbeOutcome::Fail(format!(
            "Combine body from multi-file.h is inlined into task '{name}'"
        ))),
        None => Ok(ProbeOutcome::Pass),
    }
}

/// `line_directive_present`: two-TU analyze succeeds AND some task's
/// HLS text contains a `#line` directive (natural line alignment /
/// re-snap).
fn probe_line_directive_present(env: &Env) -> Result<ProbeOutcome> {
    let work_dir = match analyze_multi_file(env)? {
        AnalyzeOutcome::Failure(reason) => return Ok(ProbeOutcome::Fail(reason)),
        AnalyzeOutcome::Success { work_dir } => work_dir,
    };
    let texts = per_task_hls_texts(work_dir.path())?;
    let has_line_directive = texts.iter().any(|(_, code)| {
        code.lines()
            .any(|line| line.trim_start().starts_with("#line"))
    });
    if has_line_directive {
        Ok(ProbeOutcome::Pass)
    } else {
        Ok(ProbeOutcome::Fail(
            "no '#line' directive in any task's HLS text".to_string(),
        ))
    }
}

/// Analyze the multi-file app with both translation units, mirroring how
/// `analyze-smoke` invokes `tapa` (see `run_analyze_app` in analyze.rs).
fn analyze_multi_file(env: &Env) -> Result<AnalyzeOutcome> {
    let inputs = [workspace_path(APP_A), workspace_path(APP_B)];
    let cflags = [
        format!("-I{}", workspace_path(APP_EXT_INCLUDE).display()),
        env.tapa_lib_flag(),
    ];
    tapa_analyze(env, "MultiFileTop", &inputs, &cflags)
}

/// Outcome of one `tapa analyze` run. `Failure` carries a one-line reason.
enum AnalyzeOutcome {
    Success { work_dir: TempDir },
    Failure(String),
}

/// Run `tapa analyze` in a fresh temp work dir. A non-zero exit is a
/// probe failure (one-line reason), while harness problems (missing tools
/// or inputs, a successful run without `tapa.json`) are fatal errors.
fn tapa_analyze(
    env: &Env,
    top: &str,
    inputs: &[PathBuf],
    cflags: &[String],
) -> Result<AnalyzeOutcome> {
    require_file(env.tapa)?;
    for input in inputs {
        require_file(input)?;
    }
    let work_dir = TempDir::with_prefix("tapa-capabilities-")
        .map_err(|error| format!("failed to create temp dir: {error}"))?;
    let mut command = Command::new(env.tapa);
    command
        .arg("--work-dir")
        .arg(work_dir.path())
        .arg("analyze");
    for input in inputs {
        command.arg("--input").arg(input);
    }
    command.arg("--top").arg(top);
    for flag in cflags {
        command.arg("--cflags").arg(flag);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {}: {error}", env.tapa.display()))?;
    if !output.status.success() {
        return Ok(AnalyzeOutcome::Failure(one_line_reason(&output)));
    }
    require_file(&work_dir.path().join("tapa.json"))?;
    Ok(AnalyzeOutcome::Success { work_dir })
}

/// One line naming why `tapa analyze` failed: the first line containing
/// "error", falling back to the first non-empty line, then the exit
/// status — enough to read a table row without dumping the whole log.
fn one_line_reason(output: &Output) -> String {
    let mut first = None;
    for stream in [&output.stderr, &output.stdout] {
        let text = String::from_utf8_lossy(stream);
        for line in text.lines().map(str::trim) {
            if line.is_empty() {
                continue;
            }
            if line.contains("error") {
                return line.to_string();
            }
            if first.is_none() {
                first = Some(line.to_string());
            }
        }
    }
    first.unwrap_or_else(|| format!("exit status {}", output.status))
}

/// Per-task HLS source text: `(task name, source)`. Schema v3 moved the
/// text out of `tapa.json` into the rewritten tree; each task's `srcs`
/// manifest names the files under `<work_dir>/rewritten/`.
fn per_task_hls_texts(work_dir: &Path) -> Result<Vec<(String, String)>> {
    let state = read_json(&work_dir.join("tapa.json"))?;
    let tasks = state
        .get("graph")
        .and_then(|graph| graph.get("tasks"))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "tapa.json missing graph.tasks".to_string())?;
    let mut texts = Vec::new();
    for (name, task) in tasks {
        let srcs = task
            .get("srcs")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| format!("task '{name}' missing array 'srcs'"))?;
        if srcs.is_empty() {
            return Err(format!("task '{name}' has empty srcs"));
        }
        for src in srcs {
            let src = src
                .as_str()
                .ok_or_else(|| format!("task '{name}': srcs entries must be strings, got {src}"))?;
            let text = std::fs::read_to_string(work_dir.join("rewritten").join(src))
                .map_err(|e| format!("task '{name}': cannot read rewritten src '{src}': {e}"))?;
            if text.is_empty() {
                return Err(format!("task '{name}': rewritten src '{src}' is empty"));
            }
            texts.push((name.clone(), text));
        }
    }
    Ok(texts)
}

fn read_graph(work_dir: &Path) -> Result<JsonValue> {
    let state = read_json(&work_dir.join("tapa.json"))?;
    state
        .get("graph")
        .cloned()
        .ok_or_else(|| "tapa.json missing 'graph'".to_string())
}

fn tasks_object(graph: &JsonValue) -> Result<&Map<String, JsonValue>> {
    graph
        .get("tasks")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "tapa.json graph missing object 'tasks'".to_string())
}

fn task_names(graph: &JsonValue) -> Result<Vec<String>> {
    Ok(tasks_object(graph)?.keys().cloned().collect())
}
