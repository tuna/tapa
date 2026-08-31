//! RTL/C++ parity baseline for the multi-file-frontend campaign.
//!
//! `parity capture` synthesizes every `tests/apps/*` kernel with the
//! current pipeline and records a sha256 manifest of the outputs at
//! `tests/apps/testdata/parity.json`; `parity check` re-runs the same
//! synthesis and diffs against the committed manifest, proving the
//! producer stays deterministic (capture is verified reproducible by
//! capturing once and checking once).
//!
//! Per app, the tool mirrors the production invocation that
//! `bazel/tapa_rules.bzl:_tapa_xo_impl` drives — one `tapa` call with
//! `analyze` chained into `synth`, `--cflags -I<app dir>` (the rule's
//! `include = ["."]`), `--target xilinx-vitis`, and the report-schema
//! stamp — minus `pack` and the mtime-reuse path: parity cares about
//! synthesis outputs only.
//!
//! Hashed per app (sha256, keys sorted):
//! - `rewritten/**` — the whole mirrored source tree, keyed by
//!   tree-relative path — RAW bytes: this is our own producer's output
//!   and stays byte-accountable;
//! - `hls/<task>/verilog/**` for every task, keyed `<task>/<file>`
//!   relative to `<work>/hls/<task>/verilog` — CANONICAL bytes: both
//!   the recorded file names and the contents pass through the
//!   normalizer in [`canonical`], which erases the source-line numbers
//!   Vitis HLS bakes into generated names and sorts the lines (details
//!   and residual blind spots there). Artifacts whose normalized paths
//!   collide are keyed as a sorted multiset, never silently merged. The
//!   sibling `report/` dir is excluded on purpose: Vitis report files
//!   embed run dates.
//!
//! This is an operator tool, not a bazel test target: a full capture is
//! hours of vendor HLS runtime. Run it from the workspace root after
//! `bazel build //tapa-core:tapa //tools:test_tools`:
//!
//! ```text
//! bazel-bin/tools/bin/tapa-test-tools parity capture
//! bazel-bin/tools/bin/tapa-test-tools parity check --apps vadd
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::common::{workspace_path, Result};

mod canonical;

/// Device `bazel/VARS.bzl:XILINX_PART_NUM` gives `tapa_xo` targets that
/// name neither a part nor a platform — what every `tests/apps` kernel
/// is synthesized for in CI.
const DEFAULT_PART_NUM: &str = "xcu55c-fsvh2892-2L-e";

/// Clock period `_tapa_xo_impl` passes alongside the default part.
const CLOCK_PERIOD: &str = "3.33";

/// Where the committed baseline lives, relative to the workspace root.
const MANIFEST_REL_PATH: &str = "tests/apps/testdata/parity.json";

const DEFAULT_JOBS: u32 = 4;

/// One `tests/apps` kernel: `<name>.cpp` with top task `<top>`. `top`
/// comes from each app's `BUILD.bazel` (`tapa_app_test(top_name = ...)`),
/// except `ignore` and `templated`, which declare no `tapa_app_test`;
/// their tops are what the `//tapa-core:tapacc_conformance_test` corpus
/// drives.
struct ParityApp {
    name: &'static str,
    top: &'static str,
}

const PARITY_APPS: &[ParityApp] = &[
    ParityApp {
        name: "async_mmap",
        top: "AsyncTop",
    },
    ParityApp {
        name: "bandwidth",
        top: "Bandwidth",
    },
    ParityApp {
        name: "cannon",
        top: "Cannon",
    },
    ParityApp {
        name: "gemv",
        top: "Gemv",
    },
    ParityApp {
        name: "graph",
        top: "Graph",
    },
    ParityApp {
        name: "ignore",
        top: "IgnoreTop",
    },
    ParityApp {
        name: "jacobi",
        top: "Jacobi",
    },
    ParityApp {
        name: "network",
        top: "Network",
    },
    ParityApp {
        name: "templated",
        top: "TemplatedTop",
    },
    ParityApp {
        name: "vadd",
        top: "VecAdd",
    },
];

impl ParityApp {
    /// Workspace-relative kernel source, `<app dir>/<name>.cpp` — the
    /// `src` every `tapa_app_test`'s `tapa_xo` target names.
    fn source(&self) -> String {
        format!("tests/apps/{}/{}.cpp", self.name, self.name)
    }
}

/// The committed baseline. `apps` maps an app to the hashes of its
/// synthesized artifacts. Maps are `BTreeMap`s so the serialized file is
/// key-sorted and byte-stable across regenerations.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParityManifest {
    vitis_version: String,
    part_num: String,
    clock_period: String,
    apps: BTreeMap<String, AppHashes>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppHashes {
    cpp: BTreeMap<String, String>,
    verilog: BTreeMap<String, String>,
}

enum Mode {
    Capture,
    Check,
}

struct Options {
    mode: Mode,
    apps: Vec<&'static ParityApp>,
    jobs: u32,
    keep: bool,
    part_num: String,
}

pub fn parity(args: &[OsString]) -> Result<()> {
    let options = parse_options(args)?;
    let tapa = locate_tapa()?;
    let version = vitis_version()?;
    println!(
        "[parity] tapa {} | vitis {version} | part {} | clock {CLOCK_PERIOD}",
        tapa.display(),
        options.part_num
    );
    match options.mode {
        Mode::Capture => capture(&options, &tapa, &version),
        Mode::Check => check(&options, &tapa, &version),
    }
}

fn parse_options(args: &[OsString]) -> Result<Options> {
    let usage = "usage: tapa-test-tools parity <capture|check> \
[--apps a,b,c] [--jobs N] [--part-num PART] [--keep]";
    let Some(mode_name) = args.first().and_then(|arg| arg.to_str()) else {
        return Err(usage.to_string());
    };
    let mode = match mode_name {
        "capture" => Mode::Capture,
        "check" => Mode::Check,
        _ => return Err(usage.to_string()),
    };

    let mut apps_arg: Option<String> = None;
    let mut jobs = DEFAULT_JOBS;
    let mut keep = false;
    let mut part_num = DEFAULT_PART_NUM.to_string();

    let mut index = 1;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| usage.to_string())?
            .to_string();
        let value = || -> Result<String> {
            let value = args.get(index + 1).and_then(|arg| arg.to_str());
            value
                .map(str::to_string)
                .ok_or_else(|| format!("{usage}\n{flag} requires a value"))
        };
        match flag.as_str() {
            "--apps" => apps_arg = Some(value()?),
            "--jobs" => {
                jobs = value()?
                    .parse()
                    .map_err(|_| format!("{usage}\n--jobs must be a positive integer"))?;
                if jobs < 1 {
                    return Err(format!("{usage}\n--jobs must be a positive integer"));
                }
            }
            "--part-num" => part_num = value()?,
            "--keep" if matches!(mode, Mode::Capture) => {
                keep = true;
                index += 1;
                continue;
            }
            _ => return Err(format!("{usage}\nunknown flag {flag}")),
        }
        index += 2;
    }

    let apps = match apps_arg {
        None => PARITY_APPS.iter().collect(),
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                PARITY_APPS
                    .iter()
                    .find(|app| app.name == name)
                    .ok_or_else(|| {
                        format!(
                            "unknown app '{name}'; valid apps: {}",
                            PARITY_APPS
                                .iter()
                                .map(|app| app.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    if apps.is_empty() {
        return Err(format!("{usage}\n--apps selected no apps"));
    }

    Ok(Options {
        mode,
        apps,
        jobs,
        keep,
        part_num,
    })
}

/// Locate the `tapa` CLI the way `analyze.rs` does (runfiles-aware),
/// plus the operator flow: `bazel build //tapa-core:tapa` stages the
/// wrapper with its runfiles at `bazel-bin/tapa-core/tapa`, which this
/// tool's own runfiles cannot see.
///
/// The result is made absolute (symlinks unresolved) on purpose: the
/// wrapper's runfiles fallback resolves its sibling tools relative to
/// `$0`, and a relative `$0` makes `find_resource` hand `synth`
/// relative `-isystem` paths that Vitis cannot resolve from its project
/// cwd — synthesis then dies on `'tapa.h' file not found`.
fn locate_tapa() -> Result<PathBuf> {
    let candidates = [
        workspace_path("tapa-core/tapa"),
        PathBuf::from("bazel-bin/tapa-core/tapa"),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return std::path::absolute(candidate)
                .map_err(|error| format!("failed to absolutize {}: {error}", candidate.display()));
        }
    }
    Err(format!(
        "cannot locate the tapa CLI (tried {} and {}); \
run `bazel build //tapa-core:tapa` and invoke this tool from the workspace root",
        candidates[0].display(),
        candidates[1].display()
    ))
}

/// The Vitis installation root, `XILINX_HLS` before `XILINX_VITIS` —
/// the precedence `tapa` itself uses (`tapa-xilinx/src/runtime/
/// process.rs`). Synthesis is impossible without it.
fn vitis_root() -> Result<PathBuf> {
    for var in ["XILINX_HLS", "XILINX_VITIS"] {
        if let Some(value) = env::var_os(var).filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(value));
        }
    }
    Err(
        "XILINX_HLS / XILINX_VITIS not set: synthesis needs the Vitis \
toolchain; source its settings64.sh first"
            .to_string(),
    )
}

/// Manifest `vitis_version`: the `YYYY.N` component of the install path
/// (e.g. `/opt/…/2025.2/Vitis` → `2025.2`), or the whole path when no
/// component looks like a version. Cheap, deterministic, and the same
/// install tree tapa resolves settings from.
fn vitis_version() -> Result<String> {
    let root = vitis_root()?;
    for component in root.components().rev() {
        if let Some(name) = component.as_os_str().to_str() {
            let mut parts = name.split('.');
            if let (Some(major), Some(minor), None) = (parts.next(), parts.next(), parts.next()) {
                let digits =
                    |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
                if digits(major) && digits(minor) {
                    return Ok(name.to_string());
                }
            }
        }
    }
    Ok(root.display().to_string())
}

/// Synthesize `app` into `work_dir`, mirroring `_tapa_xo_impl`'s argv
/// (analyze chained into synth) minus `pack` and mtime-based reuse.
/// Stdio is inherited so the operator sees live HLS progress.
fn synth_app(app: &ParityApp, tapa: &Path, work_dir: &Path, options: &Options) -> Result<()> {
    let source = workspace_path(&app.source());
    let source_dir = source
        .parent()
        .ok_or_else(|| format!("source has no parent: {}", source.display()))?;
    if !source.is_file() {
        return Err(format!("missing kernel source {}", source.display()));
    }

    let mut command = Command::new(tapa);
    let status = command
        .arg("--work-dir")
        .arg(work_dir)
        .arg("analyze")
        .arg("--input")
        .arg(&source)
        .arg("--top")
        .arg(app.top)
        .arg("--cflags")
        .arg(format!("-I{}", source_dir.display()))
        .arg("--target")
        .arg("xilinx-vitis")
        .arg("synth")
        // Part of the production argv even though the reports it stamps
        // are outside the hash set; kept so the mirror stays exact.
        .arg("--override-report-schema-version")
        .arg("redacted")
        .arg("--jobs")
        .arg(options.jobs.to_string())
        .arg("--part-num")
        .arg(&options.part_num)
        .arg("--clock-period")
        .arg(CLOCK_PERIOD)
        .status()
        .map_err(|error| format!("failed to run {}: {error}", tapa.display()))?;
    if !status.success() {
        return Err(format!(
            "tapa analyze+synth failed for {} (exit {})",
            app.name, status
        ));
    }
    Ok(())
}

/// Hash one app's synthesis outputs under `work_dir`.
///
/// Verilog keys and contents are canonicalized (see [`canonical`]);
/// files whose names canonicalize to the same key are keyed as a sorted
/// multiset (see [`canonical::keyed_multiset`]) instead of silently
/// merging — that would mask exactly the drift this gate exists to
/// catch.
fn hash_app(work_dir: &Path, app_name: &str) -> Result<AppHashes> {
    // The mirrored source tree, keyed by tree-relative path: the producer's
    // own output, hashed raw so byte drift anywhere in the tree trips the
    // gate.
    let mut cpp = BTreeMap::new();
    let mut stack = vec![work_dir.join("rewritten")];
    while let Some(dir) = stack.pop() {
        for entry in sorted_entries(&dir, app_name)? {
            if entry.is_dir() {
                stack.push(entry);
                continue;
            }
            let name = entry
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("{app_name}: non-UTF-8 name in rewritten/"))?;
            let prefix = dir
                .strip_prefix(work_dir)
                .map_err(|_| {
                    format!(
                        "{app_name}: rewritten/ escapes the work dir: {}",
                        dir.display()
                    )
                })?
                .to_string_lossy()
                .into_owned();
            let key = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            cpp.insert(key, hash_file(&entry, app_name)?);
        }
    }
    if cpp.is_empty() {
        return Err(format!(
            "{app_name}: analyze produced no rewritten/ source tree"
        ));
    }

    let mut verilog = BTreeMap::new();
    let hls_dir = work_dir.join("hls");
    for entry in sorted_entries(&hls_dir, app_name)? {
        if !entry.is_dir() {
            return Err(format!(
                "{app_name}: unexpected non-directory in hls/: {}",
                entry.display()
            ));
        }
        let task = entry
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{app_name}: non-UTF-8 task dir in hls/"))?;
        // Tasks with `synth = "ignore"` are never run through HLS and
        // legitimately have no `verilog/` dir.
        let verilog_dir = entry.join("verilog");
        if verilog_dir.is_dir() {
            hash_tree(&verilog_dir, &format!("{task}/"), &mut verilog, app_name)?;
        }
    }
    if verilog.is_empty() {
        return Err(format!("{app_name}: synth produced no hls/*/verilog files"));
    }
    Ok(AppHashes { cpp, verilog })
}

fn hash_tree(
    dir: &Path,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
    ctx: &str,
) -> Result<()> {
    // Normalized path -> canonical content hashes of every artifact on
    // it; keyed_multiset turns each group into stable manifest keys.
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    hash_tree_inner(dir, prefix, &mut grouped, ctx)?;
    out.extend(canonical::keyed_multiset(grouped));
    Ok(())
}

fn hash_tree_inner(
    dir: &Path,
    prefix: &str,
    grouped: &mut BTreeMap<String, Vec<String>>,
    ctx: &str,
) -> Result<()> {
    for entry in sorted_entries(dir, ctx)? {
        let name = entry
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{ctx}: non-UTF-8 name under {}", dir.display()))?;
        if entry.is_dir() {
            hash_tree_inner(&entry, &format!("{prefix}{name}/"), grouped, ctx)?;
        } else {
            let key = format!("{prefix}{}", canonical::canonical_verilog(name));
            grouped
                .entry(key)
                .or_default()
                .push(hash_verilog_file(&entry, ctx)?);
        }
    }
    Ok(())
}

fn sorted_entries(dir: &Path, ctx: &str) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|error| format!("{ctx}: failed to read {}: {error}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

/// Raw sha256 of a file — used for the rewritten C++, which is our own
/// producer's output and must stay byte-accountable.
fn hash_file(path: &Path, ctx: &str) -> Result<String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("{ctx}: failed to read {}: {error}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

/// Canonical sha256 of one verilog file: contents normalized and line
/// sorted (see [`canonical`]) before hashing, so line-layout-only
/// differences between generations of the same design hash equal.
fn hash_verilog_file(path: &Path, ctx: &str) -> Result<String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("{ctx}: failed to read {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{ctx}: {} is not UTF-8 verilog: {error}", path.display()))?;
    Ok(sha256_hex(
        canonical::canonical_verilog_contents(text).as_bytes(),
    ))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

/// Synthesize and hash every selected app into `sink`. The temp base is
/// leaked (with a pointer to it) whenever anything fails, so the failed
/// app's HLS outputs survive for inspection.
fn synth_and_hash_all(
    options: &Options,
    tapa: &Path,
    sink: &mut BTreeMap<String, AppHashes>,
) -> Result<()> {
    let base = TempDir::with_prefix("tapa-parity-")
        .map_err(|error| format!("failed to create temp dir: {error}"))?;
    let total = Instant::now();
    for (index, app) in options.apps.iter().enumerate() {
        println!(
            "[parity] synthesizing {} ({}/{})",
            app.name,
            index + 1,
            options.apps.len()
        );
        let work_dir = base.path().join(app.name);
        let started = Instant::now();
        let hashes = match synth_app(app, tapa, &work_dir, options)
            .and_then(|()| hash_app(&work_dir, app.name))
        {
            Ok(hashes) => hashes,
            Err(error) => {
                let kept = base.keep();
                return Err(format!("{error}; work dir kept at {}", kept.display()));
            }
        };
        println!(
            "[parity] {}: {} cpp + {} verilog files hashed ({})",
            app.name,
            hashes.cpp.len(),
            hashes.verilog.len(),
            format_duration(started.elapsed())
        );
        sink.insert(app.name.to_string(), hashes);
    }
    if options.keep {
        let kept = base.keep();
        println!("[parity] work dirs kept at {}", kept.display());
    }
    println!("[parity] total {}", format_duration(total.elapsed()));
    Ok(())
}

fn capture(options: &Options, tapa: &Path, version: &str) -> Result<()> {
    let manifest_path = manifest_path()?;
    let mut manifest = match load_manifest(&manifest_path)? {
        Some(existing) => {
            require_env_matches(&existing, options, version, "capture", &manifest_path)?;
            existing
        }
        None => ParityManifest {
            vitis_version: version.to_string(),
            part_num: options.part_num.clone(),
            clock_period: CLOCK_PERIOD.to_string(),
            apps: BTreeMap::new(),
        },
    };

    synth_and_hash_all(options, tapa, &mut manifest.apps)?;

    save_manifest(&manifest_path, &manifest)?;
    println!(
        "[parity] wrote {} ({} apps)",
        MANIFEST_REL_PATH,
        manifest.apps.len()
    );
    Ok(())
}

fn check(options: &Options, tapa: &Path, version: &str) -> Result<()> {
    let manifest_path = manifest_path()?;
    let manifest = load_manifest(&manifest_path)?.ok_or_else(|| {
        format!(
            "no parity manifest at {}; run `parity capture` first",
            manifest_path.display()
        )
    })?;
    require_env_matches(&manifest, options, version, "check", &manifest_path)?;

    let mut fresh = BTreeMap::new();
    synth_and_hash_all(options, tapa, &mut fresh)?;

    let mut report = Vec::new();
    let mut drift = 0;
    for app in &options.apps {
        let baseline = manifest.apps.get(app.name).ok_or_else(|| {
            format!(
                "app '{}' is not in {}; capture it first",
                app.name, MANIFEST_REL_PATH
            )
        })?;
        drift += diff_app(app.name, baseline, &fresh[app.name], &mut report);
    }
    for line in &report {
        println!("{line}");
    }
    if drift > 0 {
        return Err(format!(
            "parity drift: {drift} file(s) differ from {MANIFEST_REL_PATH}"
        ));
    }
    println!("[parity] OK: all {} apps match", options.apps.len());
    Ok(())
}

/// `capture` refuses to splice a baseline captured under a different
/// toolchain; `check` cannot interpret one.
fn require_env_matches(
    manifest: &ParityManifest,
    options: &Options,
    version: &str,
    action: &str,
    manifest_path: &Path,
) -> Result<()> {
    if manifest.vitis_version == version
        && manifest.part_num == options.part_num
        && manifest.clock_period == CLOCK_PERIOD
    {
        return Ok(());
    }
    Err(format!(
        "{action} refused: {} was captured with vitis {}, part {}, clock {}; \
this run has vitis {version}, part {}, clock {CLOCK_PERIOD}. Regenerate the \
full baseline in one environment",
        manifest_path.display(),
        manifest.vitis_version,
        manifest.part_num,
        manifest.clock_period,
        options.part_num
    ))
}

/// Count and report every hash difference between the committed
/// baseline and a fresh synthesis of one app.
fn diff_app(app: &str, baseline: &AppHashes, fresh: &AppHashes, report: &mut Vec<String>) -> usize {
    diff_map(app, "cpp", &baseline.cpp, &fresh.cpp, report)
        + diff_map(app, "verilog", &baseline.verilog, &fresh.verilog, report)
}

fn diff_map(
    app: &str,
    kind: &str,
    baseline: &BTreeMap<String, String>,
    fresh: &BTreeMap<String, String>,
    report: &mut Vec<String>,
) -> usize {
    let mut drift = 0;
    for (key, baseline_hash) in baseline {
        match fresh.get(key) {
            Some(fresh_hash) if fresh_hash == baseline_hash => {}
            Some(fresh_hash) => {
                report.push(format!(
                    "{app}: {kind}/{key}: hash drift (manifest {baseline_hash}, fresh {fresh_hash})"
                ));
                drift += 1;
            }
            None => {
                report.push(format!("{app}: {kind}/{key}: missing from fresh synthesis"));
                drift += 1;
            }
        }
    }
    for key in fresh.keys().filter(|key| !baseline.contains_key(*key)) {
        report.push(format!(
            "{app}: {kind}/{key}: unexpected in fresh synthesis"
        ));
        drift += 1;
    }
    drift
}

fn manifest_path() -> Result<PathBuf> {
    // Deliberately cwd-relative, not runfiles-aware: parity is an
    // operator tool whose output must land in the workspace tree.
    if !Path::new("tests/apps").is_dir() {
        return Err(
            "tests/apps not found in the current directory; run from the workspace root"
                .to_string(),
        );
    }
    Ok(PathBuf::from(MANIFEST_REL_PATH))
}

fn load_manifest(path: &Path) -> Result<Option<ParityManifest>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("invalid {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

fn save_manifest(path: &Path, manifest: &ParityManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("failed to serialize manifest: {error}"))?;
    fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn diff_map_reports_drift_missing_and_unexpected() {
        let baseline = hashes(&[("Add", "aaa"), ("Top", "bbb")]);
        let fresh = hashes(&[("Add", "zzz"), ("Extra", "ccc")]);
        let mut report = Vec::new();
        assert_eq!(diff_map("vadd", "cpp", &baseline, &fresh, &mut report), 3);
        assert_eq!(
            report,
            vec![
                "vadd: cpp/Add: hash drift (manifest aaa, fresh zzz)".to_string(),
                "vadd: cpp/Top: missing from fresh synthesis".to_string(),
                "vadd: cpp/Extra: unexpected in fresh synthesis".to_string(),
            ]
        );
        // Identical maps are silence: the common case must stay quiet.
        let mut quiet = Vec::new();
        assert_eq!(diff_map("vadd", "cpp", &baseline, &baseline, &mut quiet), 0);
        assert!(quiet.is_empty());
    }

    // Two distinct loop modules at different source lines but the same
    // loop id normalize to one path: they must hash as a sorted
    // multiset under `#<ordinal>` keys — count and contents survive,
    // and sabotage of either body still moves the manifest.
    #[test]
    fn hash_tree_keys_colliding_names_as_a_multiset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vdir = dir.path().join("verilog");
        fs::create_dir_all(&vdir).expect("mkdir");
        fs::write(
            vdir.join("Add_Add_Pipeline_VITIS_LOOP_38_1.v"),
            "module a(); assign x = icmp_ln38; endmodule\n",
        )
        .expect("write");
        fs::write(
            vdir.join("Add_Add_Pipeline_VITIS_LOOP_39_1.v"),
            "module b(); assign y = 1'b1; endmodule\n",
        )
        .expect("write");
        let mut out = BTreeMap::new();
        hash_tree(&vdir, "Add/", &mut out, "vadd").expect("hash");
        let keys: Vec<&String> = out.keys().collect();
        assert_eq!(
            keys,
            vec![
                &"Add/Add_Add_Pipeline_VITIS_LOOP_~_1.v#1".to_string(),
                &"Add/Add_Add_Pipeline_VITIS_LOOP_~_1.v#2".to_string(),
            ],
            "colliding names must key as a two-entry multiset: {keys:?}"
        );

        // Sabotage one colliding body: the multiset must drift.
        fs::write(
            vdir.join("Add_Add_Pipeline_VITIS_LOOP_39_1.v"),
            "module b(); assign y = 1'b0; endmodule\n",
        )
        .expect("rewrite");
        let mut sabotaged = BTreeMap::new();
        hash_tree(&vdir, "Add/", &mut sabotaged, "vadd").expect("hash");
        assert_ne!(out, sabotaged, "constant sabotage must register as drift");

        // Remove one loop module of the pair: the multiset shrinks.
        fs::remove_file(vdir.join("Add_Add_Pipeline_VITIS_LOOP_39_1.v")).expect("remove");
        let mut shrunk = BTreeMap::new();
        hash_tree(&vdir, "Add/", &mut shrunk, "vadd").expect("hash");
        assert_eq!(
            shrunk.keys().next(),
            Some(&"Add/Add_Add_Pipeline_VITIS_LOOP_~_1.v".to_string()),
            "a singleton path keeps the plain key"
        );
    }

    // Byte-identical files on one normalized path stay two entries:
    // deleting one of them must register as drift, not as a no-op.
    #[test]
    fn hash_tree_keeps_multiplicity_of_identical_colliding_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vdir = dir.path().join("verilog");
        fs::create_dir_all(&vdir).expect("mkdir");
        let body = "module a(); assign x = icmp_ln38; endmodule\n";
        for name in [
            "Add_Add_Pipeline_VITIS_LOOP_38_1.v",
            "Add_Add_Pipeline_VITIS_LOOP_39_1.v",
        ] {
            fs::write(vdir.join(name), body).expect("write");
        }
        let mut both = BTreeMap::new();
        hash_tree(&vdir, "Add/", &mut both, "vadd").expect("hash");
        assert_eq!(both.len(), 2);
        assert_eq!(
            both.values().collect::<Vec<_>>(),
            vec![&both["Add/Add_Add_Pipeline_VITIS_LOOP_~_1.v#1"]; 2],
            "identical artifacts hash equal but stay two entries"
        );
        fs::remove_file(vdir.join("Add_Add_Pipeline_VITIS_LOOP_39_1.v")).expect("remove");
        let mut one = BTreeMap::new();
        hash_tree(&vdir, "Add/", &mut one, "vadd").expect("hash");
        assert_ne!(both, one, "losing one of two identical files must drift");
    }

    #[test]
    fn manifest_serialization_is_sorted_and_round_trips() {
        // Inserted out of order; the committed file must still be sorted
        // and a round-trip must not move a byte.
        let manifest = ParityManifest {
            vitis_version: "2025.2".to_string(),
            part_num: DEFAULT_PART_NUM.to_string(),
            clock_period: CLOCK_PERIOD.to_string(),
            apps: [
                (
                    "vadd",
                    AppHashes {
                        cpp: hashes(&[("Add", "a")]),
                        verilog: hashes(&[("vadd/Add.v", "b")]),
                    },
                ),
                (
                    "async_mmap",
                    AppHashes {
                        cpp: hashes(&[("Writer", "c")]),
                        verilog: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        };
        let text = serde_json::to_string_pretty(&manifest).unwrap();
        let reloaded: ParityManifest = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string_pretty(&reloaded).unwrap(), text);
        assert!(text.find("\"async_mmap\"").unwrap() < text.find("\"vadd\"").unwrap());
    }
}
