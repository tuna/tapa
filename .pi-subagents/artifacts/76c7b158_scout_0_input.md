# Task for scout

Explore the C++ side of the repo at /home/dotkrnl/tapa: directories tapacc/ (clang frontend + codegen), tapa-lib/, tapa-cpp/, tapa-system-include/. Use plain `grep -rn` instead of `rg` if rg fails. Context: this is a new hardware-design language toolchain with NO users yet and NO backward-compatibility requirement; legacy support should be deleted as though it never existed. Identify: (1) compat shims/fallbacks — grep -rni for 'legacy', 'fallback', 'compat', 'deprecat', 'backward'; e.g. tapacc/codegen/xilinx.cpp around line 269 'Mirrors the legacy' comment and disaggregated `_s_*` member ports; (2) dead code: unused or duplicated headers in tapa-system-include, stub decls (tapacc/frontend/tapa_stub_decls.h), unused frontend helpers, superseded codegen paths, test files for removed features; (3) duplication / missing single source of truth: port-naming rules, width conventions, task metadata formats duplicated between tapacc C++ and the Rust crates under tapa-core (name the specific duplicated conventions and files); (4) stale comments. For each finding: file:line, 1-2 sentence description, severity (high/medium/low). Return a structured list only, no fixes.

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/76c7b158/context.md
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