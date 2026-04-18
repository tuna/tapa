//! `tapa_core.xilinx` — thin `PyO3` wrapper over the `tapa-xilinx` crate.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "error mapping lists only the SSH/remote variants that need prefixing"
)]
#![allow(
    clippy::doc_markdown,
    reason = "docs cite the Python module path literally"
)]
//!
//! Exposes the five orchestration-level entry points named in the
//! Phase 6 plan:
//!
//! - `run_hls_task(job_json) -> str` — per-task HLS invocation with
//!   retries.
//! - `pack_xo(inputs_json) -> str` — Vivado-backed `.xo` assembly.
//! - `parse_device_info(path, overrides_json) -> str` — device info
//!   lookup.
//! - `get_cflags(kind) -> list[str]` — dispatcher on `"tapacc"` /
//!   `"tapacc_remote"` / `"remote_hls"` / `"vendor_includes"`.
//! - `sync_vendor_includes(config_json) -> str` — one-shot vendor
//!   header sync.
//!
//! Additional Python-facing functions beyond the five canonical
//! entry points are restricted to justified helpers:
//!
//! - `parse_csynth_xml(bytes) -> str` and
//!   `parse_utilization_rpt(text) -> str` — pure-data aliases that
//!   the cross-language parity suite drives directly. They replace
//!   the Python `tapa.backend` parsers in
//!   `tapa-core/tests/parity_test.py`.
//! - `emit_kernel_xml(args_json) -> str` — pure-data alias for the
//!   Rust emitter, used by `test_parity_xilinx_kernel_xml_direct`
//!   to compare against `tapa.backend.kernel_metadata.print_kernel_xml`
//!   without relying on a golden-file intermediary.
//! - `_debug_search_roots() -> list[str]` — underscore-prefixed
//!   diagnostic helper that reports `runtime::paths::search_roots()`.
//!   Kept public to accelerate regression triage for
//!   `TAPA_XILINX_BINDINGS_DIR`-anchored discovery (AC-2). Not part
//!   of the stable surface; callers must not depend on it.
//!
//! Error mapping: every `XilinxError` variant surfaces as
//! `PyValueError` with the error's `Display` string. SSH / remote
//! variants are prefixed `[ssh]` so Python callers can distinguish
//! transport failures from tool failures.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;

use std::path::PathBuf;

use serde::Deserialize;

use tapa_xilinx::{
    emit_kernel_xml, get_remote_hls_cflags, get_tapa_cflags, get_tapacc_cflags,
    parse_csynth_xml, parse_device_info, parse_utilization_rpt, redact_xo,
    run_hls_with_retry, HlsJob, KernelXmlArgs, LocalToolRunner, XilinxError,
};

/// Populate `TAPA_XILINX_BINDINGS_DIR` from the loaded `tapa_core`
/// module's `__file__`. Called lazily at the top of every wrapper so
/// we see the fully-initialized module (the one we have during
/// `register()` does not yet have `__file__` set).
fn ensure_bindings_dir(py: Python<'_>) {
    if std::env::var_os("TAPA_XILINX_BINDINGS_DIR").is_some() {
        return;
    }
    let Ok(sys) = py.import("sys") else {
        return;
    };
    let Ok(modules) = sys.getattr("modules") else {
        return;
    };
    let Ok(tapa_core) = modules.get_item("tapa_core") else {
        return;
    };
    let file = tapa_core
        .getattr("__file__")
        .ok()
        .and_then(|v| v.extract::<String>().ok())
        .or_else(|| {
            tapa_core
                .getattr("__spec__")
                .ok()
                .and_then(|spec| spec.getattr("origin").ok())
                .and_then(|v| v.extract::<String>().ok())
        });
    if let Some(f) = file {
        if let Some(dir) = std::path::Path::new(&f).parent() {
            std::env::set_var("TAPA_XILINX_BINDINGS_DIR", dir);
        }
    }
}

fn to_py_err(e: &XilinxError) -> PyErr {
    let msg = match e {
        XilinxError::SshConnect { .. }
        | XilinxError::SshMuxLost { .. }
        | XilinxError::RemoteTransfer(_) => format!("[ssh] {e}"),
        _ => e.to_string(),
    };
    PyValueError::new_err(msg)
}

/// JSON adapter struct mirroring `tapa_xilinx::HlsJob`. Python passes
/// a JSON payload; absent fields use `HlsJob::default()`.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct HlsJobJson {
    task_name: String,
    cpp_source: PathBuf,
    cflags: Vec<String>,
    target_part: String,
    top_name: String,
    clock_period: String,
    reports_out_dir: PathBuf,
    hdl_out_dir: PathBuf,
    uploads: Vec<PathBuf>,
    downloads: Vec<PathBuf>,
    other_configs: String,
    solution_name: String,
    reset_low: bool,
    auto_prefix: bool,
    max_attempts: u32,
}

impl Default for HlsJobJson {
    fn default() -> Self {
        Self {
            task_name: String::new(),
            cpp_source: PathBuf::new(),
            cflags: Vec::new(),
            target_part: String::new(),
            top_name: String::new(),
            clock_period: String::new(),
            reports_out_dir: PathBuf::new(),
            hdl_out_dir: PathBuf::new(),
            uploads: Vec::new(),
            downloads: Vec::new(),
            other_configs: String::new(),
            solution_name: String::new(),
            reset_low: true,
            auto_prefix: true,
            max_attempts: 3,
        }
    }
}

impl From<HlsJobJson> for HlsJob {
    fn from(j: HlsJobJson) -> Self {
        Self {
            task_name: j.task_name,
            cpp_source: j.cpp_source,
            cflags: j.cflags,
            target_part: j.target_part,
            top_name: j.top_name,
            clock_period: j.clock_period,
            reports_out_dir: j.reports_out_dir,
            hdl_out_dir: j.hdl_out_dir,
            uploads: j.uploads,
            downloads: j.downloads,
            other_configs: j.other_configs,
            solution_name: j.solution_name,
            reset_low: j.reset_low,
            auto_prefix: j.auto_prefix,
            transient_patterns: None,
        }
    }
}

/// Run a single Vitis HLS task via the local `ToolRunner`.
///
/// Accepts the JSON shape of `HlsJobJson` (see fields above). Returns
/// a JSON object with `csynth`, `verilog_files`, and `report_paths`
/// on success. On non-zero exit, the Rust `ToolFailure` / retry-
/// exhausted variants surface as `PyValueError`. The binding does
/// not silently fall back to Python when the flag is on; missing
/// `vitis_hls` fails with a typed `ToolFailure`.
#[pyfunction]
fn run_hls_task(py: Python<'_>, job_json: &str) -> PyResult<String> {
    ensure_bindings_dir(py);
    let parsed: HlsJobJson = serde_json::from_str(job_json)
        .map_err(|e| PyValueError::new_err(format!("run_hls_task: invalid job JSON: {e}")))?;
    let max_attempts = parsed.max_attempts.max(1);
    let job: HlsJob = parsed.into();
    let runner = LocalToolRunner::new();
    let out = run_hls_with_retry(&runner, &job, max_attempts).map_err(|e| to_py_err(&e))?;
    let payload = serde_json::json!({
        "csynth": out.csynth,
        "verilog_files": out.verilog_files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "report_paths": out.report_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "stdout": out.stdout,
        "stderr": out.stderr,
    });
    serde_json::to_string(&payload).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// JSON adapter for the `.xo` packaging path. Currently runs the
/// `redact_xo` pass only — Vivado-backed `package_xo` TCL assembly
/// is tracked separately. Passing an existing `.xo` path is the
/// expected use case; the Vivado orchestration will join up with this
/// entry point once it lands.
#[derive(Debug, Deserialize)]
struct PackXoInputs {
    kernel_out_path: String,
    /// When true (the default), run the redaction pass. Set to
    /// false if the caller redacts separately.
    #[serde(default = "default_true")]
    redact: bool,
}

const fn default_true() -> bool {
    true
}

#[pyfunction]
fn pack_xo(py: Python<'_>, inputs_json: &str) -> PyResult<String> {
    ensure_bindings_dir(py);
    let inputs: PackXoInputs = serde_json::from_str(inputs_json)
        .map_err(|e| PyValueError::new_err(format!("pack_xo: invalid inputs JSON: {e}")))?;
    let path = PathBuf::from(&inputs.kernel_out_path);
    if !path.exists() {
        return Err(PyValueError::new_err(format!(
            "pack_xo: kernel_out_path does not exist: {}",
            path.display()
        )));
    }
    if inputs.redact {
        redact_xo(&path).map_err(|e| to_py_err(&e))?;
    }
    let payload = serde_json::json!({
        "kernel_out_path": inputs.kernel_out_path,
        "redacted": inputs.redact,
    });
    serde_json::to_string(&payload).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
#[pyo3(signature = (path, overrides_json=None))]
fn parse_device_info_py(
    py: Python<'_>,
    path: &str,
    overrides_json: Option<&str>,
) -> PyResult<String> {
    ensure_bindings_dir(py);
    let (part_override, clock_override) = if let Some(js) = overrides_json {
        let v: serde_json::Value = serde_json::from_str(js)
            .map_err(|e| PyValueError::new_err(format!("overrides_json: {e}")))?;
        (
            v.get("part_num").and_then(|s| s.as_str()).map(String::from),
            v.get("clock_period")
                .and_then(|s| s.as_str())
                .map(String::from),
        )
    } else {
        (None, None)
    };
    let info = parse_device_info(
        std::path::Path::new(path),
        part_override.as_deref(),
        clock_override.as_deref(),
    )
    .map_err(|e| to_py_err(&e))?;
    serde_json::to_string(&info).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn get_cflags<'py>(py: Python<'py>, kind: &str) -> PyResult<Bound<'py, PyList>> {
    ensure_bindings_dir(py);
    let flags: Vec<String> = match kind {
        "tapacc" => get_tapacc_cflags(false),
        "tapacc_remote" => get_tapacc_cflags(true),
        "remote_hls" => get_remote_hls_cflags(),
        "tapa" => get_tapa_cflags(),
        "vendor_includes" => tapa_xilinx::get_vendor_include_paths()
            .into_iter()
            .map(|p| p.display().to_string())
            .collect(),
        _ => {
            return Err(PyValueError::new_err(format!(
                "get_cflags: unknown kind `{kind}`"
            )))
        }
    };
    PyList::new(py, flags)
}

#[pyfunction]
fn _debug_search_roots(py: Python<'_>) -> PyResult<Bound<'_, PyList>> {
    ensure_bindings_dir(py);
    let roots = tapa_xilinx::runtime::paths::debug_search_roots();
    let strs: Vec<String> = roots.iter().map(|p| p.display().to_string()).collect();
    PyList::new(py, strs)
}

#[pyfunction]
fn emit_kernel_xml_py(args_json: &str) -> PyResult<String> {
    let args: KernelXmlArgs = serde_json::from_str(args_json)
        .map_err(|e| PyValueError::new_err(format!("args_json: {e}")))?;
    emit_kernel_xml(&args).map_err(|e| to_py_err(&e))
}

#[pyfunction]
fn parse_csynth_xml_py(bytes: &[u8]) -> PyResult<String> {
    let report = parse_csynth_xml(bytes).map_err(|e| to_py_err(&e))?;
    serde_json::to_string(&report).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn parse_utilization_rpt_py(text: &str) -> PyResult<String> {
    let report = parse_utilization_rpt(text).map_err(|e| to_py_err(&e))?;
    serde_json::to_string(&report).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn sync_vendor_includes(_config_json: &str) -> PyResult<String> {
    Err(PyValueError::new_err(
        "[ssh] sync_vendor_includes: remote vendor-header sync not yet implemented",
    ))
}

pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = parent.py();
    let m = PyModule::new(py, "xilinx")?;
    // Expose the directory holding the compiled extension to Rust's
    // `runtime::paths` search path — matches Python's
    // `Path(__file__).absolute().parents` behavior for installed
    // packages. Honor a pre-existing override so callers can pin it.
    if std::env::var_os("TAPA_XILINX_BINDINGS_DIR").is_none() {
        // Resolve the compiled extension directory deterministically
        // from the parent module's `__file__` (the `tapa_core`
        // module currently being initialized). This anchor matches
        // Python's `Path(__file__).parents` lookup for installed
        // packages; falls back to `__spec__.origin` when `__file__`
        // is absent (e.g. frozen builds).
        let dir = parent
            .getattr("__file__")
            .ok()
            .and_then(|v| v.extract::<String>().ok())
            .or_else(|| {
                parent
                    .getattr("__spec__")
                    .ok()
                    .and_then(|spec| spec.getattr("origin").ok())
                    .and_then(|v| v.extract::<String>().ok())
            })
            .and_then(|file| {
                std::path::Path::new(&file)
                    .parent()
                    .map(std::path::Path::to_path_buf)
            });
        if let Some(dir) = dir {
            std::env::set_var("TAPA_XILINX_BINDINGS_DIR", dir);
        }
    }
    m.add_function(wrap_pyfunction!(run_hls_task, &m)?)?;
    m.add_function(wrap_pyfunction!(pack_xo, &m)?)?;
    m.add_function(wrap_pyfunction!(parse_device_info_py, &m)?)?;
    m.add_function(wrap_pyfunction!(get_cflags, &m)?)?;
    m.add_function(wrap_pyfunction!(sync_vendor_includes, &m)?)?;
    m.add_function(wrap_pyfunction!(parse_csynth_xml_py, &m)?)?;
    m.add_function(wrap_pyfunction!(parse_utilization_rpt_py, &m)?)?;
    m.add_function(wrap_pyfunction!(emit_kernel_xml_py, &m)?)?;
    m.add_function(wrap_pyfunction!(_debug_search_roots, &m)?)?;
    // Stable Python-facing aliases for parser/emitter helpers. These
    // are in addition to the five canonical entry points listed in
    // the plan; the aliases are used by the cross-language parity
    // suite.
    m.setattr("parse_csynth_xml", m.getattr("parse_csynth_xml_py")?)?;
    m.setattr("parse_utilization_rpt", m.getattr("parse_utilization_rpt_py")?)?;
    m.setattr("emit_kernel_xml", m.getattr("emit_kernel_xml_py")?)?;
    // Expose `parse_device_info` under its public name; keep the
    // internal function name distinct from the Rust symbol.
    m.setattr(
        "parse_device_info",
        m.getattr("parse_device_info_py")?,
    )?;
    parent.add_submodule(&m)?;
    Ok(())
}
