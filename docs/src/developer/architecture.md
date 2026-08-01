# Architecture

```admonish note
This page is the **architecture charter** for the TAPA compiler's Rust
toolchain. It describes the component map, the layer rule, and the charter
of each crate — it is a standing reference, not a changelog.
The step-by-step program that brings the code in line with this charter
lives in the working document `REFACTOR-PLAN.md` (repository
root, not yet committed).
```

## Component Map

The compiler pipeline has three parts: the C++ frontend `tapacc`, the Rust
toolchain workspace under `tapa-core/`, and the separate `fpga-runtime`
workspace that ships the simulation and on-board runtime.

```text
tapacc (C++/Clang) ──TaskGraph JSON──┐
                                     ▼
tapa-protocol   tapa-ir ──────────────── schema + transforms
     │           ▲  ▲
     │           │  └────────────── tapa-rtl (Verilog parse / mutate / emit; tree-sitter + nom)
     ▼           │
tapa-floorplan (device/graph/partition/pipeline/route/solver/xdc + DSE)
tapa-codegen (RTL assembly of the top module; flat pass modules)
     ▲
tapa-xilinx (HLS/Vitis/Vivado/XO-pack tools, local+remote runners, Vitis connectivity parser adapter)
     ▲
tapa-cli (steps: analyze → synth → floorplan → pack; chain, state via tapa.json, remote config)

fpga-runtime (separate workspace): frt (FFI staticlib) ← tapa-lib/tapa/host/frt (C++ RAII)
             frt-shm, frt-dpi{,-verilator,-xsim}, frt-cosim, frt-cbindgen (ABI drift check)
```

`tapacc` emits the task graph as JSON; `tapa-ir` is the schema both sides
agree on. The engines (`tapa-floorplan`, `tapa-codegen`) transform the IR
into a floorplan and into RTL, `tapa-xilinx` drives the vendor tools, and
`tapa-cli` orchestrates the steps and persists state in `tapa.json`.

## Load-Bearing Contract Guards

Several cross-component contracts are already guarded in CI. The
architecture preserves these seams and hardens around them; any refactor
must keep them green.

- **Versioned work state.** The `tapa.json` state file is a versioned
  schema: `WorkState` in `tapa-core/tapa-ir/src/work_state.rs` carries a
  `VERSION` constant (currently `VERSION = 3`), so old state is detected
  instead of silently misparsed.
- **Atomic state writes.** All work-directory files are written through a
  tempfile-then-rename dance in `tapa-core/tapa-cli/src/state/json.rs`, so
  readers never observe a partially written state file.
- **tapacc → IR conformance.** The task-graph schema has two
  implementations that must agree: `tapacc` emits the JSON and `tapa-ir`
  parses it with `deny_unknown_fields`.
  `tapa-core/tapa-cli/tests/tapacc_conformance.rs` runs the real `tapacc`
  on a reference design and strict-parses its verbatim stdout, catching any
  drift between the two sides.
- **Generated-ABI drift check.** The C header for the `frt` runtime is
  generated with cbindgen; `fpga-runtime/frt-cbindgen/src/main.rs` with
  `--check` regenerates it and fails if the checked-in header differs.

## Layer Rule

**Dependencies point down only.** Engines never depend on engines or on
drivers; everything that travels cross-crate goes through `tapa-ir`
contract types.

```text
L0 contracts : tapa-protocol (constants)        tapa-ir (schema, transforms, floorplan result types)
L1 model     : tapa-rtl (verilog parse/mutate/emit)
L2 engines   : tapa-floorplan ──> writes FloorplanResult │ tapa-codegen ──> reads FloorplanResult
L3 drivers   : tapa-xilinx (tools/runners/platform)      tapa-xilinx/connectivity (Vitis parser adapter)
L4 orchestr. : tapa-cli (typed pipeline, artifact registry, state)
L5 runtime   : fpga-runtime (frt + cosim + dpi)  │ tapacc (C++ frontend)  — side contracts into L0
```

`tapacc` and `fpga-runtime` sit at L5 as side contracts into L0: they
exchange data with the toolchain only through the versioned schemas defined
in `tapa-ir` and `tapa-protocol`, never by sharing implementation.

## Crate Charters

- **tapa-ir** — vendor-neutral schema plus pure transforms. No I/O
  adapters, no tool logic. It owns the JSON wire format the C++ frontend
  and the Rust toolchain agree on, and the typed `FloorplanResult` that
  connects the two engines.
- **tapa-rtl** — the only place Verilog text is parsed, mutated, or
  emitted (tree-sitter + nom). Other crates never manipulate Verilog
  source as text.
- **tapa-floorplan** — turns `(design, device)` into a serialized
  `FloorplanResult`. It knows nothing about RTL text or the CLI.
- **tapa-codegen** — a pure function
  `(Design, Option<FloorplanResult>, modules) → ArtifactManifest` whose
  manifest is the complete file set: generated RTL, template files, FSM
  files, and embedded assets. Packaging is then a copy operation for the
  caller.
- **tapa-xilinx** — every external tool invocation, discovery, versioning,
  and transport (local and remote runners) lives here. No other crate
  shells out to vendor tools.
- **tapa-cli** — no domain logic. Orchestration, state persistence, the
  artifact registry, and UX only. Steps declare what they read and write
  over the artifact registry.

## Conventions

- **Naming disambiguation.** Where crate types collide, alias once at the
  consuming module instead of importing ambiguous names at each use
  site: `tapa_ir::Port as IrPort` versus `tapa_rtl::Port as RtlPort`
  (the one live collision today, in `tapa-codegen`'s RTL state);
  `tapa_cli::remote_config` (config/flag overlay) versus
  `tapa_xilinx::runtime::remote` (transport).
- **Error taxonomy.** Each crate defines its domain errors with
  `thiserror` in one `error.rs` per crate. Engines return structured
  errors; only the CLI renders user-facing prose.
- **State.** `tapa.json` is the only inter-step persistence. All new
  persisted fields go through the versioned `WorkState` schema — never
  ad-hoc JSON between steps.
- **File-size budgets.** Soft budget of ~800 lines per file; `lib.rs` is
  re-exports only, ≤ ~100 lines. When a module family grows past three
  files it moves into a directory. Budgets are enforced in review, not CI.
- **Test placement.** Unit tests live in-source next to the code;
  behavior and conformance suites live in `tests/`; golden comparisons are
  normalized (sorted, trimmed) so refactors stay cheap.

## Public API Surface Audit

CI gates on `cargo public-api` over the L0/L1 crates (`tapa-protocol`,
`tapa-ir`, `tapa-rtl`), diffing the generated listing against blessed
baselines under `docs/api/` (promoted from advisory to required in
Phase 5). Drift blocks the branch: either revert the accidental change
or, for an intentional API change, re-bless the baseline in the same
PR. To regenerate a baseline:

```bash
rustup toolchain install nightly-2026-07-30 --profile minimal --component rust-docs-json
cargo +nightly-2026-07-30 install cargo-public-api --locked --version 0.52.0
cd tapa-core
cargo public-api --package tapa-ir > ../docs/api/tapa-ir.public-api.txt
```

(repeat for `tapa-protocol` and `tapa-rtl`).

## The Phased Program

This charter is the target; the code is brought in line with it
incrementally by the phased refactor program (invariants and charters, then
per-engine restructuring, then runtime hardening), where each phase
preserves observable behavior and lands behind the conformance guards
above. The program itself — findings, phase contents, exit criteria, and
risks — lives in the working document **`REFACTOR-PLAN.md`**
(repository root, not yet committed).
