# Task for scout

Explore /home/dotkrnl/tapa/tapa-core: crates tapa-rtl, tapa-ir, tapa-protocol. This is a new HDL toolchain with NO users and NO backward-compatibility requirements. Identify: (1) backward-compat shims and fallbacks (grep for legacy|fallback|backward|deprecat), e.g. singleton-array port fallbacks in module.rs, raw-attribute parser fallbacks, legacy canonical instance names; (2) dead/unused code (pub items never used, cfg-gated relics, commented-out code); (3) duplication suggesting a missing single source of truth (same constants/regexes/naming conventions repeated across crates, e.g. port naming conventions like `_s_dout`, reset handling); (4) stale comments/docs referencing removed features. For each finding give file:line, a 1-2 sentence description, and severity. Return a structured list, no fixes.

---
Update progress at: /home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/535e8132/parallel-0/0-scout/context.md
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