# Task for scout

Explore /home/dotkrnl/tapa/fpga-runtime (Rust workspace: frt, frt-cosim, frt-shm, and any others) plus /home/dotkrnl/tapa/bazel/dpi_rules.bzl and bazel/dependencies.bzl ONLY as far as xsim_legacy_rdi/xsc_legacy_rdi relate to fpga-runtime. New toolchain, no users, no backward compat needed. Identify: (1) the 'legacy' xsim RDI path end-to-end: FRT_XSIM_LEGACY env var, legacy flags in xsim tb/runner, run_cosim.tcl.j2 {% if legacy %}, metadata sax_control legacy localparam parsing + comment fallback, match on Xsim { legacy } in frt/src/instance.rs, and the bazel xsim_legacy_rdi/dpi_legacy_rdi_library rules; (2) other fallbacks (libfrt_dpi suffix fallback in frt/src/cosim/mod.rs, fallback_stream_dir in xo.rs); (3) dead code, unused env vars/features; (4) duplication (metadata parsed in multiple places, clock/port names re-derived). For each: file:line, description, severity, and whether removing the entire legacy-RDI feature looks safe (what references it). Structured list, no fixes.

---
Update progress at: /home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/535e8132/parallel-0/3-scout/context.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: attested
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Return concrete findings with file paths and severity when applicable

Required evidence: review-findings, residual-risks

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
`criteriaSatisfied[].status` must be exactly one of: satisfied, not-satisfied, not-applicable.
`commandsRun[].result` must be exactly one of: passed, failed, not-run.
`manualNotes` and `notes` are optional strings; an empty string means no note and does not satisfy `manual-notes` evidence.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```