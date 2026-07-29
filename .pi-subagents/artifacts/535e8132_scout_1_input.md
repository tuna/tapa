# Task for scout

Explore /home/dotkrnl/tapa/tapa-core: crates tapa-codegen and tapa-floorplan only. This is a new toolchain with NO users and NO backward-compat requirements. Identify: (1) backward-compat shims/fallbacks (grep legacy|fallback|preserving|non-floorplanned), e.g. 'legacy parent reset' handling in children.rs, legacy RTL byte-for-byte preservation in distributed_control.rs, '_peek suffix fallbacks', fifo topology fallbacks, ilp.rs 'lhs:rhs legacy spelling' parsing; (2) dead code and test-only scaffolding for removed features; (3) duplication/missing single source of truth (reset distribution logic replicated, port-naming conventions repeated across codegen modules, constants duplicated between crates); (4) golden-file tests that pin 'legacy' behavior that would only exist for compat. For each: file:line, 1-2 sentence description, severity. Structured list, no fixes.

---
Update progress at: /home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md

---
**Output:**
Write your findings to exactly this path: /home/dotkrnl/tapa/.pi-subagents/artifacts/outputs/535e8132/parallel-0/1-scout/context.md
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