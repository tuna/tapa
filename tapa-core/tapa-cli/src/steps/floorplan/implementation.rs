//! Isolated implementation and frequency selection for floorplan candidates.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use tapa_floorplan::device::select::select_device;
use tapa_ir::{Design, FloorplanResult, Target, WorkState};
use tapa_xilinx::{
    run_vitis_link, target_frequency_mhz, ToolRunner, VitisLinkJob, VitisLinkOutput,
};

use crate::error::{CliError, Result};
use crate::state::json;
use crate::steps::pack::vitis_packaging::package_prepared_vitis_rtl;
use crate::steps::synth::rtl_codegen::{emit_prepared_rtl_tree, prepare_rtl_state, TaskHdlInputs};

use super::{FLOORPLAN_CONNECTIVITY, FLOORPLAN_XDC};

pub(super) const IMPLEMENTATION_TIMING_REPORT: &str = "floorplan-timing.rpt";
pub(super) const IMPLEMENTATION_METRICS: &str = "floorplan-metrics.json";

const CANDIDATE_ROOT: &str = "dse";
const CANDIDATE_DIAGNOSTICS: &str = "candidates.json";
const CANDIDATE_DIAGNOSTIC: &str = "candidate.json";

/// One exact plan ready for isolated RTL generation and implementation.
#[derive(Debug, Clone)]
pub(super) struct PlannedCandidate {
    pub index: usize,
    pub requested_utilization_cap: f64,
    pub effective_block_utilization_cap: Option<f64>,
    pub multilevel_block_margin_applied: bool,
    pub utilization_cap_policy: UtilizationCapPolicy,
    pub realized_max_utilization: f64,
    pub floorplan: FloorplanResult,
}

impl PlannedCandidate {
    fn cap_summary(&self, precision: usize) -> String {
        cap_summary(
            self.utilization_cap_policy,
            self.requested_utilization_cap,
            self.effective_block_utilization_cap,
            self.multilevel_block_margin_applied,
            precision,
        )
    }
}

/// Whether the planner may raise a requested cap when it is infeasible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum UtilizationCapPolicy {
    Relaxing,
    Exact,
}

impl UtilizationCapPolicy {
    const fn label(self) -> &'static str {
        match self {
            Self::Relaxing => "relaxing",
            Self::Exact => "exact",
        }
    }
}

/// A planning rejection retained in the deterministic diagnostic sequence.
#[derive(Debug, Clone, Copy)]
pub(super) struct InfeasibleCandidate {
    pub index: usize,
    pub requested_utilization_cap: f64,
    pub effective_block_utilization_cap: f64,
    pub multilevel_block_margin_applied: bool,
}

impl InfeasibleCandidate {
    fn cap_summary(&self, precision: usize) -> String {
        cap_summary(
            UtilizationCapPolicy::Exact,
            self.requested_utilization_cap,
            Some(self.effective_block_utilization_cap),
            self.multilevel_block_margin_applied,
            precision,
        )
    }
}

fn cap_summary(
    policy: UtilizationCapPolicy,
    logic_cap: f64,
    block_cap: Option<f64>,
    multilevel_block_margin_applied: bool,
    precision: usize,
) -> String {
    let Some(block_cap) = block_cap else {
        return format!("{} logic cap {logic_cap:.precision$}", policy.label());
    };
    let margin = if multilevel_block_margin_applied {
        ", multilevel block margin applied"
    } else {
        ""
    };
    format!(
        "{} logic cap {logic_cap:.precision$}, effective block cap {block_cap:.precision$}{margin}",
        policy.label(),
    )
}

/// Inputs validated once before any solver or implementation process starts.
#[derive(Debug, Clone)]
pub(super) struct ImplementationTarget {
    platform: String,
    target_mhz: u32,
    vivado_threads: u32,
}

impl ImplementationTarget {
    pub(super) const fn target_mhz(&self) -> u32 {
        self.target_mhz
    }
}

/// Validate the implementation-only parts of the persisted synthesis target.
pub(super) fn validate_target(
    state: &WorkState,
    vivado_threads: u32,
) -> Result<ImplementationTarget> {
    if state.graph.target != Target::XilinxVitis {
        return Err(CliError::InvalidArg(
            "`--run-impl` and `--dse` require the `xilinx-vitis` target".to_string(),
        ));
    }

    let part_num = state.flow.part_num.as_deref().ok_or_else(|| {
        CliError::InvalidArg(
            "implementation requires a synthesized target part; rerun `tapa synth`".to_string(),
        )
    })?;
    let device = select_device(part_num).map_err(|error| CliError::Floorplan(error.to_string()))?;
    if device.is_versal {
        return Err(CliError::InvalidArg(format!(
            "implementation from `tapa floorplan` is not supported for Versal device `{}`",
            device.key,
        )));
    }

    let platform = state.flow.platform.as_deref().ok_or_else(|| {
        CliError::InvalidArg(
            "implementation requires synthesis with `--platform <installed-platform-name>`"
                .to_string(),
        )
    })?;
    if !is_logical_platform_name(platform) {
        return Err(CliError::InvalidArg(format!(
            "implementation platform `{platform}` must be an installed platform name, not a path",
        )));
    }

    let clock_period = state.flow.clock_period.ok_or_else(|| {
        CliError::InvalidArg(
            "implementation requires a synthesized target clock period; rerun `tapa synth`"
                .to_string(),
        )
    })?;
    let target_mhz = target_frequency_mhz(clock_period.nanoseconds())
        .map_err(|error| CliError::InvalidArg(error.to_string()))?;

    Ok(ImplementationTarget {
        platform: platform.to_string(),
        target_mhz,
        vivado_threads,
    })
}

/// Measure the largest realized resource fraction in a completed floorplan.
pub(super) fn realized_max_utilization(floorplan: &FloorplanResult) -> Result<f64> {
    let device =
        select_device(&floorplan.device).map_err(|error| CliError::Floorplan(error.to_string()))?;
    tapa_floorplan::dse::maximum_realized_utilization(floorplan, &device)
        .map_err(|error| CliError::Floorplan(error.to_string()))
}

fn is_logical_platform_name(platform: &str) -> bool {
    !platform.is_empty()
        && platform == platform.trim()
        && !platform.contains(['/', '\\'])
        && !platform.to_ascii_lowercase().ends_with(".xpfm")
}

/// Implement candidates in a bounded pool, then invoke `publish` exactly once
/// for the highest-frequency success. The callback is never invoked when all
/// candidates fail.
#[allow(
    clippy::too_many_arguments,
    reason = "candidate execution needs the immutable design, generated-input, target, and publication boundaries explicitly"
)]
pub(super) fn implement_and_publish(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    state: &WorkState,
    flat: &Design,
    hdl_inputs: &TaskHdlInputs,
    connectivity: Option<&[u8]>,
    target: &ImplementationTarget,
    candidates: Vec<PlannedCandidate>,
    infeasible: &[InfeasibleCandidate],
    jobs: usize,
    publish: impl FnOnce(&ImplementationWinner) -> Result<()>,
) -> Result<()> {
    let worker_count = candidate_worker_count(jobs, candidates.len())?;
    recreate_candidate_root(work_dir)?;

    log::info!(
        "floorplan implementation: {} feasible and {} infeasible candidate(s), {} worker(s)",
        candidates.len(),
        infeasible.len(),
        worker_count,
    );
    let outcomes = match worker_count {
        0 => Vec::new(),
        1 => candidates
            .into_iter()
            .map(|candidate| {
                implement_one(
                    runner,
                    work_dir,
                    state,
                    flat,
                    hdl_inputs,
                    connectivity,
                    target,
                    candidate,
                )
            })
            .collect(),
        _ => crate::util::run_in_pool(
            worker_count,
            "DSE implementation",
            CliError::Floorplan,
            || {
                candidates
                    .into_par_iter()
                    .map(|candidate| {
                        implement_one(
                            runner,
                            work_dir,
                            state,
                            flat,
                            hdl_inputs,
                            connectivity,
                            target,
                            candidate,
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )?,
    };

    persist_diagnostics(work_dir, &outcomes, infeasible)?;
    finish_with_winner(outcomes, infeasible, publish)
}

fn candidate_worker_count(requested_jobs: usize, candidate_count: usize) -> Result<usize> {
    if requested_jobs == 0 {
        return Err(CliError::InvalidArg(
            "DSE implementation jobs must be greater than zero".to_string(),
        ));
    }
    Ok(requested_jobs.min(candidate_count))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the per-candidate boundary keeps every filesystem and tool input explicit"
)]
fn implement_one(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    state: &WorkState,
    flat: &Design,
    hdl_inputs: &TaskHdlInputs,
    connectivity: Option<&[u8]>,
    target: &ImplementationTarget,
    candidate: PlannedCandidate,
) -> CandidateOutcome {
    let outcome = match try_implement_one(
        runner,
        work_dir,
        state,
        flat,
        hdl_inputs,
        connectivity,
        target,
        &candidate,
    ) {
        Ok(output) if output.timing.fmax_mhz.is_finite() => {
            CandidateOutcome::Success(ImplementationWinner { candidate, output })
        }
        Ok(output) => CandidateOutcome::Failure {
            candidate,
            error: format!(
                "implementation reported non-finite Fmax `{}` MHz",
                output.timing.fmax_mhz
            ),
        },
        Err(error) => CandidateOutcome::Failure {
            candidate,
            error: error.to_string(),
        },
    };
    match &outcome {
        CandidateOutcome::Success(success) => log::info!(
            "floorplan candidate_{:03}: implementation completed at {:.3} MHz",
            success.candidate.index,
            success.output.timing.fmax_mhz,
        ),
        CandidateOutcome::Failure { candidate, .. } => log::warn!(
            "floorplan candidate_{:03}: implementation failed; diagnostics will be stored in {}",
            candidate.index,
            candidate_root(work_dir, candidate.index).display(),
        ),
    }
    outcome
}

#[allow(
    clippy::too_many_arguments,
    reason = "the per-candidate boundary keeps every filesystem and tool input explicit"
)]
fn try_implement_one(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    state: &WorkState,
    flat: &Design,
    hdl_inputs: &TaskHdlInputs,
    connectivity: Option<&[u8]>,
    target: &ImplementationTarget,
    candidate: &PlannedCandidate,
) -> Result<VitisLinkOutput> {
    let paths = CandidatePaths::new(work_dir, candidate.index, &flat.top);
    paths.recreate()?;

    log::info!(
        "floorplan candidate_{:03} [1/3]: preparing RTL ({}, realized max utilization {:.3})",
        candidate.index,
        candidate.cap_summary(3),
        candidate.realized_max_utilization,
    );
    let mut rtl_state = prepare_rtl_state(flat, hdl_inputs)?;
    rtl_state.floorplan = Some(candidate.floorplan.clone());
    emit_prepared_rtl_tree(&paths.root, &mut rtl_state, hdl_inputs)?;

    let xdc = tapa_floorplan::render_xdc(&candidate.floorplan)
        .map_err(|error| CliError::Floorplan(error.to_string()))?;
    json::write_bytes_atomic(&paths.constraints, FLOORPLAN_XDC, xdc.as_bytes())?;
    if let Some(bytes) = connectivity {
        json::write_bytes_atomic(&paths.constraints, FLOORPLAN_CONNECTIVITY, bytes)?;
    }

    let mut candidate_state = state.clone();
    candidate_state.graph = flat.clone();
    candidate_state.floorplan = Some(candidate.floorplan.clone());
    log::info!(
        "floorplan candidate_{:03} [2/3]: packaging XO with Vivado",
        candidate.index,
    );
    let xo = package_prepared_vitis_rtl(runner, &candidate_state, &paths.rtl, &paths.xo, None)?;

    log::info!(
        "floorplan candidate_{:03} [3/3]: running Vitis implementation",
        candidate.index,
    );
    let job = VitisLinkJob::builder()
        .kernel_name(flat.top.clone())
        .xo(xo)
        .platform(target.platform.clone())
        .target_mhz(target.target_mhz)
        .vivado_threads(target.vivado_threads)
        .work_dir(crate::util::utf8(&paths.link))
        .artifacts_dir(crate::util::utf8(&paths.artifacts))
        .output_xclbin(crate::util::utf8(&paths.xclbin))
        .report_dir(crate::util::utf8(&paths.reports))
        .log_dir(crate::util::utf8(&paths.logs))
        .temp_dir(crate::util::utf8(&paths.temp))
        .floorplan_xdc(Some(crate::util::utf8(
            paths.constraints.join(FLOORPLAN_XDC),
        )))
        .connectivity_config(
            connectivity.map(|_| crate::util::utf8(paths.constraints.join(FLOORPLAN_CONNECTIVITY))),
        )
        .build();
    Ok(run_vitis_link(runner, &job)?)
}

#[derive(Debug)]
enum CandidateOutcome {
    Success(ImplementationWinner),
    Failure {
        candidate: PlannedCandidate,
        error: String,
    },
}

/// Winning plan and its implementation artifacts.
#[derive(Debug)]
pub(super) struct ImplementationWinner {
    candidate: PlannedCandidate,
    output: VitisLinkOutput,
}

impl ImplementationWinner {
    pub(super) fn floorplan(&self) -> &FloorplanResult {
        &self.candidate.floorplan
    }

    pub(super) fn publish_artifacts(&self, work_dir: &Path, target_mhz: u32) -> Result<()> {
        atomic_copy(
            &self.output.timing_report,
            &work_dir.join(IMPLEMENTATION_TIMING_REPORT),
        )?;
        let metrics = WinnerMetrics {
            candidate_index: self.candidate.index,
            requested_utilization_cap: self.candidate.requested_utilization_cap,
            effective_block_utilization_cap: self.candidate.effective_block_utilization_cap,
            multilevel_block_margin_applied: self.candidate.multilevel_block_margin_applied,
            utilization_cap_policy: self.candidate.utilization_cap_policy,
            realized_max_utilization: self.candidate.realized_max_utilization,
            target_mhz,
            reported_target_period_ns: self.output.timing.reported_target_period_ns,
            reported_target_mhz: self.output.timing.reported_target_mhz,
            wns_ns: self.output.timing.wns_ns,
            achieved_period_ns: self.output.timing.achieved_period_ns,
            fmax_mhz: self.output.timing.fmax_mhz,
            timing_report: IMPLEMENTATION_TIMING_REPORT,
        };
        let mut bytes = serde_json::to_vec_pretty(&metrics)?;
        bytes.push(b'\n');
        json::write_bytes_atomic(work_dir, IMPLEMENTATION_METRICS, &bytes)
    }

    pub(super) fn log_selection(&self, work_dir: &Path) {
        log::info!(
            "selected floorplan candidate_{:03}: {}, realized max utilization {:.3}, achieved Fmax {:.3} MHz; metrics at {}",
            self.candidate.index,
            self.candidate.cap_summary(3),
            self.candidate.realized_max_utilization,
            self.output.timing.fmax_mhz,
            work_dir.join(IMPLEMENTATION_METRICS).display(),
        );
    }
}

fn finish_with_winner(
    mut outcomes: Vec<CandidateOutcome>,
    infeasible: &[InfeasibleCandidate],
    publish: impl FnOnce(&ImplementationWinner) -> Result<()>,
) -> Result<()> {
    let winner = winner_index(&outcomes).ok_or_else(|| all_failed_error(&outcomes, infeasible))?;
    let CandidateOutcome::Success(winner) = outcomes.swap_remove(winner) else {
        unreachable!("winner selection only returns successful candidates")
    };
    publish(&winner)
}

fn winner_index(outcomes: &[CandidateOutcome]) -> Option<usize> {
    let mut best: Option<(usize, usize, f64)> = None;
    for (position, outcome) in outcomes.iter().enumerate() {
        let CandidateOutcome::Success(success) = outcome else {
            continue;
        };
        let fmax = success.output.timing.fmax_mhz;
        if !fmax.is_finite() {
            continue;
        }
        let index = success.candidate.index;
        if best.is_none_or(|(_, best_index, best_fmax)| {
            matches!(fmax.total_cmp(&best_fmax), Ordering::Greater)
                || (matches!(fmax.total_cmp(&best_fmax), Ordering::Equal) && index < best_index)
        }) {
            best = Some((position, index, fmax));
        }
    }
    best.map(|(position, _, _)| position)
}

fn all_failed_error(outcomes: &[CandidateOutcome], infeasible: &[InfeasibleCandidate]) -> CliError {
    let mut failures = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            CandidateOutcome::Success(_) => None,
            CandidateOutcome::Failure { candidate, error } => Some((
                candidate.index,
                format!(
                    "candidate_{:03} ({}): {error}",
                    candidate.index,
                    candidate.cap_summary(9),
                ),
            )),
        })
        .chain(infeasible.iter().map(|candidate| {
            (
                candidate.index,
                format!(
                    "candidate_{:03} ({}): exact planning infeasible",
                    candidate.index,
                    candidate.cap_summary(9),
                ),
            )
        }))
        .collect::<Vec<_>>();
    failures.sort_by_key(|(index, _)| *index);
    let details = failures
        .into_iter()
        .map(|(_, message)| message)
        .collect::<Vec<_>>()
        .join("; ");
    CliError::Floorplan(if details.is_empty() {
        "no floorplan candidates were available for implementation".to_string()
    } else {
        format!("no floorplan candidate completed implementation: {details}")
    })
}

fn persist_diagnostics(
    work_dir: &Path,
    outcomes: &[CandidateOutcome],
    infeasible: &[InfeasibleCandidate],
) -> Result<()> {
    let dse_root = work_dir.join(CANDIDATE_ROOT);
    fs_err::create_dir_all(&dse_root)?;
    let mut diagnostics = outcomes
        .iter()
        .map(CandidateDiagnostic::from_outcome)
        .chain(infeasible.iter().map(CandidateDiagnostic::from_infeasible))
        .collect::<Vec<_>>();
    diagnostics.sort_by_key(|diagnostic| diagnostic.index);

    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.status != CandidateStatus::Infeasible)
    {
        let candidate_root = candidate_root(work_dir, diagnostic.index);
        fs_err::create_dir_all(&candidate_root)?;
        write_json(&candidate_root, CANDIDATE_DIAGNOSTIC, diagnostic)?;
        if let Some(CandidateOutcome::Success(success)) = outcomes
            .iter()
            .find(|outcome| outcome.candidate().index == diagnostic.index)
        {
            json::write_bytes_atomic(
                &candidate_root,
                "vitis.stdout.log",
                success.output.stdout.as_bytes(),
            )?;
            json::write_bytes_atomic(
                &candidate_root,
                "vitis.stderr.log",
                success.output.stderr.as_bytes(),
            )?;
        }
    }
    write_json(&dse_root, CANDIDATE_DIAGNOSTICS, &diagnostics)
}

impl CandidateOutcome {
    fn candidate(&self) -> &PlannedCandidate {
        match self {
            Self::Success(success) => &success.candidate,
            Self::Failure { candidate, .. } => candidate,
        }
    }
}

fn write_json(root: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    json::write_bytes_atomic(root, name, &bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CandidateStatus {
    Succeeded,
    Failed,
    Infeasible,
}

#[derive(Debug, Serialize)]
struct CandidateDiagnostic {
    index: usize,
    requested_utilization_cap: f64,
    effective_block_utilization_cap: Option<f64>,
    multilevel_block_margin_applied: bool,
    utilization_cap_policy: UtilizationCapPolicy,
    realized_max_utilization: Option<f64>,
    status: CandidateStatus,
    fmax_mhz: Option<f64>,
    achieved_period_ns: Option<f64>,
    error: Option<String>,
}

impl CandidateDiagnostic {
    fn from_outcome(outcome: &CandidateOutcome) -> Self {
        match outcome {
            CandidateOutcome::Success(success) => Self {
                index: success.candidate.index,
                requested_utilization_cap: success.candidate.requested_utilization_cap,
                effective_block_utilization_cap: success.candidate.effective_block_utilization_cap,
                multilevel_block_margin_applied: success.candidate.multilevel_block_margin_applied,
                utilization_cap_policy: success.candidate.utilization_cap_policy,
                realized_max_utilization: Some(success.candidate.realized_max_utilization),
                status: CandidateStatus::Succeeded,
                fmax_mhz: Some(success.output.timing.fmax_mhz),
                achieved_period_ns: Some(success.output.timing.achieved_period_ns),
                error: None,
            },
            CandidateOutcome::Failure { candidate, error } => Self {
                index: candidate.index,
                requested_utilization_cap: candidate.requested_utilization_cap,
                effective_block_utilization_cap: candidate.effective_block_utilization_cap,
                multilevel_block_margin_applied: candidate.multilevel_block_margin_applied,
                utilization_cap_policy: candidate.utilization_cap_policy,
                realized_max_utilization: Some(candidate.realized_max_utilization),
                status: CandidateStatus::Failed,
                fmax_mhz: None,
                achieved_period_ns: None,
                error: Some(error.clone()),
            },
        }
    }

    fn from_infeasible(candidate: &InfeasibleCandidate) -> Self {
        Self {
            index: candidate.index,
            requested_utilization_cap: candidate.requested_utilization_cap,
            effective_block_utilization_cap: Some(candidate.effective_block_utilization_cap),
            multilevel_block_margin_applied: candidate.multilevel_block_margin_applied,
            utilization_cap_policy: UtilizationCapPolicy::Exact,
            realized_max_utilization: None,
            status: CandidateStatus::Infeasible,
            fmax_mhz: None,
            achieved_period_ns: None,
            error: Some("exact planning infeasible".to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
struct WinnerMetrics<'a> {
    candidate_index: usize,
    requested_utilization_cap: f64,
    effective_block_utilization_cap: Option<f64>,
    multilevel_block_margin_applied: bool,
    utilization_cap_policy: UtilizationCapPolicy,
    realized_max_utilization: f64,
    target_mhz: u32,
    reported_target_period_ns: f64,
    reported_target_mhz: f64,
    wns_ns: f64,
    achieved_period_ns: f64,
    fmax_mhz: f64,
    timing_report: &'a str,
}

#[derive(Debug)]
struct CandidatePaths {
    root: PathBuf,
    rtl: PathBuf,
    package: PathBuf,
    xo: PathBuf,
    constraints: PathBuf,
    link: PathBuf,
    artifacts: PathBuf,
    xclbin: PathBuf,
    reports: PathBuf,
    logs: PathBuf,
    temp: PathBuf,
}

impl CandidatePaths {
    fn new(work_dir: &Path, index: usize, top: &str) -> Self {
        let root = candidate_root(work_dir, index);
        let rtl = root.join("rtl");
        let package = root.join("package");
        let constraints = root.join("constraints");
        let link = root.join("link");
        let artifacts = link.join("artifacts");
        Self {
            xo: package.join(format!("{top}.xo")),
            xclbin: artifacts.join(format!("{top}.xclbin")),
            reports: artifacts.join("reports"),
            logs: artifacts.join("logs"),
            temp: link.join("vitis.tmp"),
            root,
            rtl,
            package,
            constraints,
            link,
            artifacts,
        }
    }

    fn recreate(&self) -> Result<()> {
        match fs_err::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for directory in [
            &self.rtl,
            &self.package,
            &self.constraints,
            &self.link,
            &self.artifacts,
            &self.reports,
            &self.logs,
        ] {
            fs_err::create_dir_all(directory)?;
        }
        Ok(())
    }
}

fn candidate_root(work_dir: &Path, index: usize) -> PathBuf {
    work_dir
        .join(CANDIDATE_ROOT)
        .join(format!("candidate_{index:03}"))
}

fn recreate_candidate_root(work_dir: &Path) -> Result<()> {
    let root = work_dir.join(CANDIDATE_ROOT);
    match fs_err::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs_err::create_dir_all(root)?;
    Ok(())
}

fn atomic_copy(source: impl AsRef<Path>, destination: &Path) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        CliError::Floorplan(format!(
            "implementation artifact `{}` has no parent directory",
            destination.display()
        ))
    })?;
    fs_err::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut input = fs_err::File::open(source.as_ref().to_path_buf())?;
    std::io::copy(&mut input, temporary.as_file_mut())?;
    temporary.as_file_mut().sync_all()?;
    fs_err::rename(temporary.path(), destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::ClockPeriod;

    fn floorplan() -> FloorplanResult {
        FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: std::collections::BTreeMap::new(),
            routes: Vec::new(),
            slot_usage: std::collections::BTreeMap::new(),
        }
    }

    fn success(index: usize, fmax_mhz: f64) -> CandidateOutcome {
        CandidateOutcome::Success(ImplementationWinner {
            candidate: PlannedCandidate {
                index,
                requested_utilization_cap: 0.9,
                effective_block_utilization_cap: Some(1.0),
                multilevel_block_margin_applied: true,
                utilization_cap_policy: UtilizationCapPolicy::Exact,
                realized_max_utilization: 0.8,
                floorplan: floorplan(),
            },
            output: VitisLinkOutput {
                timing_report: format!("candidate_{index:03}.rpt").into(),
                timing: tapa_xilinx::KernelTiming {
                    reported_target_period_ns: 3.0,
                    reported_target_mhz: 333.0,
                    wns_ns: 0.0,
                    achieved_period_ns: 1000.0 / fmax_mhz,
                    fmax_mhz,
                },
                stdout: String::new(),
                stderr: String::new(),
            },
        })
    }

    #[test]
    fn equal_fmax_prefers_lower_candidate_index() {
        let outcomes = vec![success(4, 350.0), success(1, 350.0), success(0, 349.0)];
        let winner = winner_index(&outcomes).expect("winner");
        assert_eq!(outcomes[winner].candidate().index, 1);
    }

    #[test]
    fn all_fail_never_invokes_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active = dir.path().join(FLOORPLAN_XDC);
        let state_path = dir.path().join("tapa.json");
        let rtl_path = dir.path().join("rtl").join("Top.v");
        fs_err::create_dir_all(rtl_path.parent().expect("RTL parent")).expect("RTL directory");
        fs_err::write(&active, "previous floorplan").expect("write active marker");
        fs_err::write(&state_path, "previous state").expect("write state");
        fs_err::write(&rtl_path, "previous RTL").expect("write RTL");
        let candidate = PlannedCandidate {
            index: 0,
            requested_utilization_cap: 0.9,
            effective_block_utilization_cap: Some(1.0),
            multilevel_block_margin_applied: true,
            utilization_cap_policy: UtilizationCapPolicy::Exact,
            realized_max_utilization: 0.8,
            floorplan: floorplan(),
        };
        let outcomes = vec![CandidateOutcome::Failure {
            candidate,
            error: "injected link failure".to_string(),
        }];
        let published = std::cell::Cell::new(false);

        let error = finish_with_winner(outcomes, &[], |_| {
            published.set(true);
            fs_err::write(&active, "new floorplan")?;
            fs_err::write(&state_path, "new state")?;
            fs_err::write(&rtl_path, "new RTL")?;
            Ok(())
        })
        .expect_err("all failures must reject publication");

        assert!(!published.get());
        assert!(error.to_string().contains("candidate_000"));
        assert!(error.to_string().contains("exact logic cap 0.900000000"));
        assert!(error
            .to_string()
            .contains("effective block cap 1.000000000"));
        assert_eq!(
            fs_err::read_to_string(active).expect("read active marker"),
            "previous floorplan"
        );
        assert_eq!(
            fs_err::read_to_string(state_path).expect("read state"),
            "previous state"
        );
        assert_eq!(
            fs_err::read_to_string(rtl_path).expect("read RTL"),
            "previous RTL"
        );
    }

    #[test]
    fn candidates_use_disjoint_generated_and_tool_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = CandidatePaths::new(dir.path(), 0, "Top");
        let second = CandidatePaths::new(dir.path(), 1, "Top");

        for (left, right) in [
            (&first.rtl, &second.rtl),
            (&first.xo, &second.xo),
            (&first.temp, &second.temp),
            (&first.artifacts, &second.artifacts),
        ] {
            assert_ne!(left, right);
            assert!(left.starts_with(&first.root));
            assert!(right.starts_with(&second.root));
        }
        assert!(!first.temp.starts_with(&first.artifacts));
        assert!(!second.temp.starts_with(&second.artifacts));
    }

    #[test]
    fn candidate_batch_starts_from_a_fresh_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stale = candidate_root(dir.path(), 7).join("stale.xclbin");
        fs_err::create_dir_all(stale.parent().expect("parent")).expect("candidate dir");
        fs_err::write(&stale, "stale").expect("stale artifact");

        recreate_candidate_root(dir.path()).expect("fresh root");

        assert!(!stale.exists());
        assert!(dir.path().join(CANDIDATE_ROOT).is_dir());
    }

    #[test]
    fn candidate_workers_are_bounded_by_feasible_work() {
        assert_eq!(candidate_worker_count(8, 3).expect("worker count"), 3);
        assert_eq!(candidate_worker_count(1, 3).expect("worker count"), 1);
        assert_eq!(candidate_worker_count(8, 0).expect("empty sweep"), 0);
        candidate_worker_count(0, 3).expect_err("zero requested jobs must fail");
    }

    #[test]
    fn exact_candidate_diagnostics_record_effective_block_cap() {
        let outcome = success(0, 350.0);
        let feasible = serde_json::to_value(CandidateDiagnostic::from_outcome(&outcome))
            .expect("serialize feasible diagnostic");
        assert_eq!(feasible["requested_utilization_cap"], 0.9);
        assert_eq!(feasible["effective_block_utilization_cap"], 1.0);
        assert_eq!(feasible["multilevel_block_margin_applied"], true);
        assert!(feasible.get("xclbin").is_none());

        let infeasible = InfeasibleCandidate {
            index: 1,
            requested_utilization_cap: 0.8,
            effective_block_utilization_cap: 0.9,
            multilevel_block_margin_applied: true,
        };
        let infeasible = serde_json::to_value(CandidateDiagnostic::from_infeasible(&infeasible))
            .expect("serialize infeasible diagnostic");
        assert_eq!(infeasible["requested_utilization_cap"], 0.8);
        assert_eq!(infeasible["effective_block_utilization_cap"], 0.9);
        assert_eq!(infeasible["multilevel_block_margin_applied"], true);
        assert!(infeasible.get("xclbin").is_none());
    }

    #[test]
    fn relaxing_candidate_metrics_name_requested_and_realized_utilization() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_timing = dir.path().join("candidate.rpt");
        fs_err::write(&source_timing, "timing").expect("write timing");
        let CandidateOutcome::Success(mut winner) = success(0, 350.0) else {
            unreachable!("success helper returns a winner")
        };
        winner.candidate.requested_utilization_cap = 0.7;
        winner.candidate.effective_block_utilization_cap = None;
        winner.candidate.multilevel_block_margin_applied = false;
        winner.candidate.utilization_cap_policy = UtilizationCapPolicy::Relaxing;
        winner.candidate.realized_max_utilization = 0.82;
        winner.output.timing_report = crate::util::utf8(source_timing);

        winner
            .publish_artifacts(dir.path(), 300)
            .expect("publish metrics");
        let metrics: serde_json::Value = serde_json::from_slice(
            &fs_err::read(dir.path().join(IMPLEMENTATION_METRICS)).expect("read metrics"),
        )
        .expect("parse metrics");

        assert_eq!(metrics["requested_utilization_cap"], 0.7);
        assert!(metrics["effective_block_utilization_cap"].is_null());
        assert_eq!(metrics["multilevel_block_margin_applied"], false);
        assert_eq!(metrics["utilization_cap_policy"], "relaxing");
        assert_eq!(metrics["realized_max_utilization"], 0.82);
        assert!(metrics.get("usage_limit").is_none());
        assert!(metrics.get("max_utilization").is_none());
        assert!(metrics.get("xclbin").is_none());
        assert_eq!(metrics["timing_report"], IMPLEMENTATION_TIMING_REPORT);
    }

    #[test]
    fn realized_utilization_uses_the_most_constrained_resource() {
        let mut floorplan = floorplan();
        floorplan.slot_usage.insert(
            tapa_floorplan::device::Coor::slot(0, 0).region_name(),
            tapa_ir::Area {
                lut: 110_400,
                ff: 110_400,
                bram_18k: 0,
                dsp: 0,
                uram: 0,
            },
        );

        let realized = realized_max_utilization(&floorplan).expect("valid utilization");

        assert!((realized - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn implementation_target_requires_vitis_named_alveo_platform_and_clock() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {"Top": {"readable_name":"Top", "code":"", "level":"lower",
                    "synth":"hls", "ports":[], "self_area":{"lut":1}}}
            }"#,
        )
        .expect("graph");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.platform = Some("xilinx_u280_test".to_string());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        let target = validate_target(&state, 2).expect("valid target");
        assert_eq!(target.platform, "xilinx_u280_test");
        assert_eq!(target.target_mhz, 300);

        for platform in ["", "/opt/xilinx/platform", "platform.xpfm"] {
            state.flow.platform = Some(platform.to_string());
            validate_target(&state, 2).expect_err("platform path must fail");
        }

        state.flow.platform = Some("xilinx_u280_test".to_string());
        // A non-numeric period is no longer representable; zero is the one
        // unusable value the typed field still admits.
        state.flow.clock_period = Some(ClockPeriod::ZERO);
        validate_target(&state, 2).expect_err("invalid clock must fail");
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state.flow.part_num = Some("xcvc1902-vsva2197-2MP-e-S".to_string());
        validate_target(&state, 2).expect_err("Versal must fail early");
    }
}
