# Task for scout

Explore repo-root infra of /home/dotkrnl/tapa: MODULE.bazel, VARS.bzl, bazel/ dir, .github/ workflows, tools/, install.sh, docs/ (structure only, not prose quality), tests/ layout, .bazelrc, .pre-commit-config.yaml. New toolchain, no users, no backward compat. Identify: (1) legacy tool version support: XILINX_TOOL_LEGACY_PATH/VERSION 2022.2 in VARS.bzl, xsim_legacy_rdi repos in MODULE.bazel/dependencies.bzl — everything that would need removal if legacy support is dropped; (2) single-source-of-truth violations: version numbers, tool paths, dependency pins duplicated across files (e.g. VERSION file vs elsewhere, rust versions, bazel module versions repeated); (3) dead/disabled CI jobs, obsolete scripts in tools/, unused bazel rules/macros, stale install.sh branches; (4) docs sections documenting features that no longer exist or legacy workflows (list specific doc files/sections). For each: file:line, description, severity. Structured list, no fixes.

---
Update progress at: /home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/535e8132/parallel-0/5-scout/context.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

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