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
//! - `pack_xo(inputs_json) -> str` — `.xo` reproducibility pass.
//!   Currently runs the `redact_xo` normalization over an existing
//!   archive (timestamps + report/XML payload redactions). The full
//!   Vivado-backed `package_xo` TCL composition is tracked
//!   separately and will land here without changing the Python
//!   call surface.
//! - `parse_device_info(path, overrides_json) -> str` — device info
//!   lookup.
//! - `get_cflags(kind) -> list[str]` — dispatcher on `"tapacc"` /
//!   `"tapacc_remote"` / `"remote_hls"` / `"vendor_includes"`.
//! - `sync_vendor_includes(config_json) -> str` — one-shot vendor
//!   header sync.
//!
//! The public `tapa_core.xilinx` module exposes **only** those five
//! entry points — `dir(tapa_core.xilinx)` lists exactly those names
//! (plus the `_internal` submodule described below).
//!
//! Internal (underscore-prefixed) submodule `tapa_core.xilinx._internal`
//! carries test-only helpers that the cross-language parity suite uses
//! to drive Rust parsers/emitters directly. These are **not** part of
//! the stable surface and are not accessible as top-level attributes
//! of `tapa_core.xilinx`:
//!
//! - `_internal.parse_csynth_xml(bytes) -> str`
//! - `_internal.parse_utilization_rpt(text) -> str`
//! - `_internal.emit_kernel_xml(args_json) -> str`
//! - `_internal.parse_device_info(path, overrides_json=None) -> str`
//! - `_internal.debug_search_roots() -> list[str]` — triage helper.
//!
//! Python callers that need these must import them from
//! `tapa_core.xilinx._internal` and are warned in that module's
//! docstring that the surface can change without a deprecation cycle.
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
    pack_xo as pack_xo_core, parse_csynth_xml, parse_device_info, parse_utilization_rpt,
    redact_xo, run_hls_with_retry, sync_remote_vendor_includes, DeviceInfo, HlsJob,
    KernelXmlArgs, LocalToolRunner, PackageXoInputs, RemoteConfig, SshMuxOptions, SshSession,
    XilinxError,
};
use std::sync::Arc;

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

const fn default_true() -> bool {
    true
}

const fn default_max_attempts() -> u32 {
    3
}

/// JSON adapter struct mirroring `tapa_xilinx::HlsJob`.
///
/// Required fields (no default): `task_name`, `cpp_source`,
/// `target_part`, `top_name`, `clock_period`, `reports_out_dir`,
/// `hdl_out_dir`. Missing required fields fail JSON-parse with a
/// schema error before the tool is invoked.
///
/// Optional knobs: `cflags`, `uploads`, `downloads`, `other_configs`,
/// `solution_name`, `reset_low` (default `true`), `auto_prefix`
/// (default `true`), `max_attempts` (default `3`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HlsJobJson {
    task_name: String,
    cpp_source: PathBuf,
    #[serde(default)]
    cflags: Vec<String>,
    target_part: String,
    top_name: String,
    clock_period: String,
    reports_out_dir: PathBuf,
    hdl_out_dir: PathBuf,
    #[serde(default)]
    uploads: Vec<PathBuf>,
    #[serde(default)]
    downloads: Vec<PathBuf>,
    #[serde(default)]
    other_configs: String,
    #[serde(default)]
    solution_name: String,
    #[serde(default = "default_true")]
    reset_low: bool,
    #[serde(default = "default_true")]
    auto_prefix: bool,
    #[serde(default = "default_max_attempts")]
    max_attempts: u32,
    /// Optional remote config. When present, `run_hls_task` dispatches
    /// through `RemoteToolRunner` instead of `LocalToolRunner`. The
    /// JSON shape matches `tapa_xilinx::RemoteConfig` so Python
    /// callers can pass the same dict they already read from
    /// `~/.taparc` or `VARS.local.bzl`.
    #[serde(default)]
    remote: Option<RemoteConfig>,
}

impl HlsJobJson {
    fn into_job(self) -> (HlsJob, Option<RemoteConfig>) {
        let remote = self.remote.clone();
        (
            HlsJob {
                task_name: self.task_name,
                cpp_source: self.cpp_source,
                cflags: self.cflags,
                target_part: self.target_part,
                top_name: self.top_name,
                clock_period: self.clock_period,
                reports_out_dir: self.reports_out_dir,
                hdl_out_dir: self.hdl_out_dir,
                uploads: self.uploads,
                downloads: self.downloads,
                other_configs: self.other_configs,
                solution_name: self.solution_name,
                reset_low: self.reset_low,
                auto_prefix: self.auto_prefix,
                transient_patterns: None,
            },
            remote,
        )
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
        .map_err(|e| {
            PyValueError::new_err(format!(
                "run_hls_task: invalid job JSON: {e}"
            ))
        })?;
    let max_attempts = parsed.max_attempts.max(1);
    let (job, remote_cfg) = parsed.into_job();
    // Dispatch: remote config present → RemoteToolRunner (mirrors
    // `tapa/remote/popen.py::create_tool_process`); otherwise drive
    // the local runner. The flag-on Python call site must not lose
    // remote capability when `remote` is threaded through.
    let out = if let Some(cfg) = remote_cfg {
        let session =
            Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
        let runner = tapa_xilinx::RemoteToolRunner::new(session);
        run_hls_with_retry(&runner, &job, max_attempts)
            .map_err(|e| to_py_err(&e))?
    } else {
        let runner = LocalToolRunner::new();
        run_hls_with_retry(&runner, &job, max_attempts)
            .map_err(|e| to_py_err(&e))?
    };
    let payload = serde_json::json!({
        "csynth": out.csynth,
        "verilog_files": out.verilog_files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "report_paths": out.report_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "stdout": out.stdout,
        "stderr": out.stderr,
    });
    serde_json::to_string(&payload)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// JSON adapter for `.xo` packaging.
///
/// Two modes — the binding dispatches on the presence of `hdl_dir`:
///
/// 1. **Full pack mode** (`hdl_dir` present): drives
///    `tapa_xilinx::pack_xo` via `LocalToolRunner`. Emits `kernel.xml`,
///    formats the `package_xo` TCL, invokes Vivado, asserts the `.xo`
///    landed, then redacts. Requires `top_name`, `part_num`,
///    `clock_period`, `kernel_xml`.
///
/// 2. **Redact-only mode** (no `hdl_dir`): expects `kernel_out_path`
///    to point at an already-built `.xo` and runs the redaction pass
///    over it. Preserves the R8/R9 smoke contract for callers that
///    still produce `.xo` via the Python backend.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackXoInputs {
    kernel_out_path: String,
    /// When true (the default), run the redaction pass. Ignored in
    /// full pack mode — redaction always runs there.
    #[serde(default = "default_true")]
    redact: bool,
    /// If present, enter full pack mode and drive Vivado.
    #[serde(default)]
    hdl_dir: Option<PathBuf>,
    #[serde(default)]
    top_name: Option<String>,
    #[serde(default)]
    part_num: Option<String>,
    #[serde(default)]
    clock_period: Option<String>,
    #[serde(default)]
    kernel_xml: Option<KernelXmlArgs>,
    #[serde(default)]
    cpp_kernels: Vec<PathBuf>,
    #[serde(default)]
    m_axi_params: Vec<(String, Vec<(String, String)>)>,
    #[serde(default)]
    s_axi_ifaces: Vec<String>,
}

#[pyfunction]
fn pack_xo(py: Python<'_>, inputs_json: &str) -> PyResult<String> {
    ensure_bindings_dir(py);
    let inputs: PackXoInputs = serde_json::from_str(inputs_json)
        .map_err(|e| PyValueError::new_err(format!("pack_xo: invalid inputs JSON: {e}")))?;
    let path = PathBuf::from(&inputs.kernel_out_path);

    if let Some(hdl_dir) = inputs.hdl_dir {
        let top_name = inputs.top_name.ok_or_else(|| {
            PyValueError::new_err("pack_xo: full mode requires `top_name`")
        })?;
        let part_num = inputs.part_num.ok_or_else(|| {
            PyValueError::new_err("pack_xo: full mode requires `part_num`")
        })?;
        let clock_period = inputs.clock_period.ok_or_else(|| {
            PyValueError::new_err("pack_xo: full mode requires `clock_period`")
        })?;
        let kernel_xml = inputs.kernel_xml.ok_or_else(|| {
            PyValueError::new_err("pack_xo: full mode requires `kernel_xml`")
        })?;
        let s_axi_ifaces = if inputs.s_axi_ifaces.is_empty() {
            PackageXoInputs::default_s_axi()
        } else {
            inputs.s_axi_ifaces
        };
        let core = PackageXoInputs {
            top_name,
            hdl_dir,
            device_info: DeviceInfo {
                part_num,
                clock_period: clock_period.clone(),
            },
            clock_period,
            kernel_xml,
            kernel_out_path: path,
            cpp_kernels: inputs.cpp_kernels,
            m_axi_params: inputs.m_axi_params,
            s_axi_ifaces,
        };
        let runner = LocalToolRunner::new();
        let out = pack_xo_core(&runner, &core).map_err(|e| to_py_err(&e))?;
        let payload = serde_json::json!({
            "kernel_out_path": out.display().to_string(),
            "redacted": true,
            "mode": "full",
        });
        return serde_json::to_string(&payload)
            .map_err(|e| PyValueError::new_err(e.to_string()));
    }

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
        "mode": "redact_only",
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
fn sync_vendor_includes(py: Python<'_>, config_json: &str) -> PyResult<String> {
    ensure_bindings_dir(py);
    let cfg: RemoteConfig = serde_json::from_str(config_json)
        .map_err(|e| PyValueError::new_err(format!("sync_vendor_includes: invalid config JSON: {e}")))?;
    let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
    let cache = sync_remote_vendor_includes(session.as_ref()).map_err(|e| to_py_err(&e))?;
    let payload = serde_json::json!({
        "cache_dir": cache.display().to_string(),
    });
    serde_json::to_string(&payload).map_err(|e| PyValueError::new_err(e.to_string()))
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
    // Five canonical Python-facing entry points listed in the plan.
    m.add_function(wrap_pyfunction!(run_hls_task, &m)?)?;
    m.add_function(wrap_pyfunction!(pack_xo, &m)?)?;
    m.add_function(wrap_pyfunction!(get_cflags, &m)?)?;
    m.add_function(wrap_pyfunction!(sync_vendor_includes, &m)?)?;
    // `parse_device_info` is the fifth canonical entry point; the
    // Rust function symbol is suffixed `_py` to avoid colliding with
    // the re-exported core `parse_device_info`. Only the `parse_device_info`
    // Python name is registered.
    m.add_function(wrap_pyfunction!(parse_device_info_py, &m)?)?;
    m.setattr("parse_device_info", m.getattr("parse_device_info_py")?)?;
    m.delattr("parse_device_info_py")?;

    // Internal submodule for parity/triage helpers (not part of the
    // stable Python surface). Parity tests import from
    // `tapa_core.xilinx._internal`.
    let internal = PyModule::new(py, "_internal")?;
    internal.setattr(
        "__doc__",
        "Internal helpers for cross-language parity and triage.\n\n\
         NOT STABLE: names under this submodule may be renamed, moved, \
         or removed without a deprecation cycle. Production code must \
         use the five documented entry points on `tapa_core.xilinx`.",
    )?;
    internal.add_function(wrap_pyfunction!(parse_csynth_xml_py, &internal)?)?;
    internal.add_function(wrap_pyfunction!(parse_utilization_rpt_py, &internal)?)?;
    internal.add_function(wrap_pyfunction!(emit_kernel_xml_py, &internal)?)?;
    internal.add_function(wrap_pyfunction!(_debug_search_roots, &internal)?)?;
    internal.add_function(wrap_pyfunction!(parse_device_info_py, &internal)?)?;
    internal.setattr("parse_csynth_xml", internal.getattr("parse_csynth_xml_py")?)?;
    internal.setattr(
        "parse_utilization_rpt",
        internal.getattr("parse_utilization_rpt_py")?,
    )?;
    internal.setattr("emit_kernel_xml", internal.getattr("emit_kernel_xml_py")?)?;
    internal.setattr("parse_device_info", internal.getattr("parse_device_info_py")?)?;
    internal.setattr("debug_search_roots", internal.getattr("_debug_search_roots")?)?;
    for nm in [
        "parse_csynth_xml_py",
        "parse_utilization_rpt_py",
        "emit_kernel_xml_py",
        "parse_device_info_py",
        "_debug_search_roots",
    ] {
        internal.delattr(nm)?;
    }
    m.add_submodule(&internal)?;

    parent.add_submodule(&m)?;
    Ok(())
}
