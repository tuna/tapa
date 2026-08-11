# Hard Rules

Binding for every agent and human, whatever the task. Orientation and
setup: `README.md`; user and developer docs: `docs/src/`.

## Sponsorship & commits

**Every commit message must carry the trailer:**

```text
Co-Authored-By: Kimi K3 (1M) <noreply@moonshot.ai>
```

This project is sponsored by Kimi; the trailer is mandatory on every
commit, no exceptions. Commit style otherwise: Conventional
(`feat|fix|refactor|docs|test|build|chore`), optional scope (e.g.
`feat(tapa-floorplan): ...`), subject ≤72 chars, small and atomic.

## Model warning

**TAPA is a task-parallel dataflow HLS model — not sequential C++
semantics with different syntax.** Tasks run concurrently and communicate
through streams/mmaps with explicit handshake semantics; familiar
software or RTL instincts are not evidence and are often wrong. Derive
behavior from the source and the documented contracts in `docs/src/`;
not derivable from those authorities → stop and ask.

## Pipeline

```text
C++ source ─tapacc (Clang frontend)→ task graph/JSON
         ─tapa-core (Rust)→ IR → Floorplan → Codegen → RTL (.xo)
         ─v++→ bitstream        C++ source ─frt→ software simulation
```

- `tapacc/` — Clang-based frontend extracting the task graph.
- `tapa-core/` — Rust compiler: `tapa-ir`, `tapa-floorplan`,
  `tapa-codegen`, `tapa-rtl`, `tapa-protocol`, `tapa-xilinx`,
  orchestrated by `tapa-cli`.
- `fpga-runtime/` — `frt` runtime and cosim/DPI backends (Rust).
- `tapa-cpp/`, `tapa-lib/` — C++ headers and host library.

## Process

- Ask when unsure — never silently decide. Report blockers; no
  workarounds.
- Occam's Razor: the simplest mechanism meeting the contract; never
  multiply types, passes, crates, tests, or docs; delete speculative
  generality on sight.
- Docs are snapshots, never worklogs — a change updates every doc it
  affects in the same commit; Git is the only archive.
- Never widen the plan: out-of-scope work is a blocker to report, not an
  invitation.
- Before done: `bazel test //...` and `pre-commit run --all-files`
  (buildifier, clang-format, `cargo fmt`/`clippy` across all manifests,
  codespell — the config already encodes the golden-file exclusions).

## Where things live

- Reusable logic moves to one typed home, never copy-adapted; keep files
  and functions small.
- Compiler state is typed Rust values in `tapa-ir`; no string-keyed
  lookups where a typed reference exists.
- Golden/reference data lives in `tapa-core/testdata/` (`golden/`,
  `rtl/`, `task-graph/`, `topology/`). Byte-equality with toolchain
  output is semantic — never "fix" formatting of goldens; the pre-commit
  exclusions for `tapa-core/testdata/golden/` are deliberate.

## Evidence & gates

- Tests are evidence, not process: no TDD, quotas, or coverage targets. A
  committed test is admitted only when it is necessary (it protects a
  contract no retained test owns), reusable, likely to break under a
  plausible future change, and non-trivial. Verification that does not
  meet the bar is run once inline and never committed.
- Golden additions and updates get semantic review, never blind accepts;
  a regenerated golden that nobody read is not evidence.
- Functional tests (`tests/functional/`) exercise end-to-end stream/mmap
  semantics; regression tests (`tests/regression/`) pin real designs;
  a bug fix lands with the test that would have caught it.
- Build warnings are errors (`-Werror`, clippy); do not silence a
  warning to make a build pass — fix the cause or justify the exception
  in the commit message.
