# Task for scout

Explore /home/dotkrnl/tapa/tapa-core: crates tapa-xilinx and tapa-cli, plus tapa-core/tapa_wrapper.sh. New toolchain, NO users, NO backward compat needed. Identify: (1) fallbacks/compat (grep fallback|legacy|compat|deprecat), e.g. ap_clk fallback in timing.rs, HLS fallbacks in tools/hls/mod.rs, wrapper script source-or-fallback chains; (2) support for multiple tool versions where only one is current (Vitis 2022.2 vs newer, version sniffing, multiple output formats); (3) dead code, unused CLI flags/env vars, hidden deprecated options; (4) duplicated knowledge across files (clock names, tool paths, file naming conventions duplicated between vitis/hls/packaging code). For each: file:line, description, severity. Structured list, no fixes.

---
Update progress at: /home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/535e8132/parallel-0/2-scout/context.md
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