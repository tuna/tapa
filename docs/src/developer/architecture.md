# Architecture

This page describes the compiler and runtime as they are organized today.
Read it before moving code between components or changing a cross-component
schema. User-facing pipeline commands are covered in
[The Compile Pipeline](../concepts/compile-pipeline.md).

## Component map

TAPA has three main implementation areas:

- `tapacc/`: the C++/Clang frontend.
- `tapa-core/`: the Rust compiler workspace.
- `fpga-runtime/` and `tapa-lib/`: the Rust and C++ runtime used for software
  simulation, cosimulation, and XRT execution.

```text
C++ sources
    │
    ▼
tapacc ──task-graph JSON──► tapa-cli
                              │ analyze
                              │ synth
                              │ floorplan (optional)
                              │ pack
                              ▼
                         .xo or .zip

tapa-protocol port names, directions, widths, and handshake constants
tapa-ir       typed graph, work state, bindings, and floorplan results
tapa-rtl      Verilog parsing, transformation, and emission
tapa-codegen  connected RTL and artifact manifests
tapa-xilinx   Xilinx tool drivers, packaging, and remote transport
tapa-floorplan placement, routing, pipelining, DSE, and XDC

tapa-lib      C++ task/stream/mmap API and host runtime
frt           execution backends and C ABI used by tapa-lib
frt-cosim     archive metadata, testbenches, and simulator runners
frt-shm       shared-memory queues and DPI configuration
frt-dpi-*     xsim and Verilator DPI adapters
```

`tapacc` extracts the task graph. `tapa-cli` orchestrates compilation and
persists typed state from `tapa-ir`. The engines produce floorplans and RTL;
`tapa-xilinx` drives Xilinx tools and packages their outputs.

## Pipeline stages

1. **Analyze.** `tapa analyze` runs `tapacc` locally. The frontend rewrites
   the required source tree and emits the task graph. The CLI stores a typed
   `WorkState` in `<work-dir>/tapa.json` and keeps raw `tapacc` output in
   `tapacc.json` for debugging.
2. **Synthesize.** `tapa synth` runs Vitis HLS for each non-ignored task,
   collects reports and Verilog, and asks `tapa-codegen` for the connected RTL
   tree. For a design with AI Engine tasks it also compiles the graph with
   `aiecompiler` (unless `--skip-aie`), and HLS and codegen see only the PL
   tasks. Work can run locally or through the remote tool runner.
3. **Floorplan (optional).** `tapa floorplan` reads the synthesized graph and
   per-task areas. It places and routes the design, inserts pipeline stages,
   stores the typed floorplan in `tapa.json`, and publishes `floorplan.xdc`.
   Planning uses the external CBC solver. Implementation feedback (`--run-impl`
   or `--dse`) additionally invokes Vitis/Vivado.
4. **Pack.** `tapa pack` creates an XO for `xilinx-vitis` or a ZIP for
   `xilinx-slash` and `xilinx-hls`. Packaging includes generated RTL,
   metadata, and any accepted custom-RTL overlays. XO and SLASH packaging can
   use the remote runner; `xilinx-hls` ZIP assembly is local. A
   `xilinx-vitis` top on the AI Engine has no PL kernel, so it packs a ZIP of
   the work state, the compiled graph, and its argument list; on a VCK5000,
   TAPA builds the top's PL kernel, which packs as an XO.
5. **Link (optional).** `tapa link` turns a package into an `.xclbin`
   through `v++` or a `.vbin` through SLASH.

`tapa compile` composes analyze, synth, and pack. Floorplanning is a separate
step between synth and pack.

## Crate responsibilities and layer rule

Dependencies point toward contracts and models, not sideways between engines
or up into the CLI. Cross-component data uses typed values rather than
string-keyed maps or ad hoc JSON.

| Component | Owns | Must not own |
|---|---|---|
| `tapa-protocol` | Port names, directions, widths, and handshake constants | Compiler state or vendor-tool policy |
| `tapa-ir` | Task-graph and work-state schemas, bindings, transforms, clock/area types, `FloorplanResult` | File layout, RTL text, tool invocation |
| `tapa-rtl` | Verilog CST/parsing, typed mutation, and emission | Task graph orchestration or packaging |
| `tapa-floorplan` | Device models, placement, routing, pipelining, DSE, and XDC rendering | Verilog rewriting or CLI policy |
| `tapa-codegen` | Conversion from IR and parsed modules to a complete RTL artifact manifest | Process execution and output packaging |
| `tapa-xilinx` | Xilinx tool discovery and invocation, platform/XML handling, XO packaging, local/remote runners, SSH transport | Vendor-neutral transforms or floorplan algorithms |
| `tapa-cli` | Command parsing, step ordering, work-directory layout, state I/O, reports, and user-facing errors | Duplicate engine algorithms |

The external CBC process used by `tapa-floorplan` is intentionally separate
from the Xilinx tool drivers in `tapa-xilinx`. Likewise, `tapacc` is a frontend
process, not a Xilinx tool invocation.

## Work-state and frontend contracts

### One typed state document

`<work-dir>/tapa.json` is the only JSON task-graph and flow state file shared
between compiler steps. `WorkState` has a version stamp, and the nested
`TaskGraph` has its own frontend schema version. Readers reject unsupported
versions instead of attempting to interpret stale data.

The original task hierarchy remains authoritative. `flow.flatten_hierarchy`
selects physical RTL lowering without replacing that semantic graph. Generated
RTL, constraints, reports, and metadata are fixed-name artifacts around the
state file, not additional task-graph schemas.

The CLI writes the state file and its small JSON side artifacts atomically
through `tapa-core/tapa-cli/src/state/json.rs`. Vendor tools still write their
own reports and projects directly.

### `tapacc` ↔ `tapa-ir` conformance

The C++ frontend and Rust IR must agree on every schema field and invocation
binding. `tapa-ir` uses strict deserialization, including unknown-field
rejection and full-document validation.

`//tapa-core:tapacc_conformance_test` runs the real frontend on reference
kernels and parses its output with the production Rust types. This catches
schema drift that hand-written fixtures cannot detect.

## Runtime boundaries

The C++ public API lives in `tapa-lib`. For host execution, it uses the `frt`
C ABI for artifact-specific execution while retaining the same task, stream,
and mmap interfaces for software simulation.

- `frt-shm` owns shared-memory queue geometry and the DPI configuration
  exchanged with simulator processes.
- `frt-cosim` reads the packed `WorkState`, derives kernel argument and stream
  metadata, generates testbenches, and launches xsim or Verilator.
- `frt-dpi`, `frt-dpi-xsim`, and `frt-dpi-verilator` marshal AXI and stream
  transfers between simulator signal layouts and host tokens.
- `frt` selects fast cosim (`.xo`/`.zip`), XRT (`.xclbin`), VRT (`.vbin`),
  or AI Engine simulation (a `tapa compile --aie-sim` package) from the
  artifact path and tracks buffer transfer, execution, completion, and
  timing.

`frt-cosim` depends on `tapa-ir` and `tapa-protocol` as its contract crates,
so archive schema changes are checked by Rust types across the two Cargo
workspaces. It does not depend on compiler engines or the CLI.

The generated C header is another guarded boundary:
`frt/tests/c_api_header_test.rs` compares the checked-in `c_api.h` with fresh
cbindgen output. Regenerate it with:

```bash
cd fpga-runtime
cargo run -p frt --example gen_c_api > ../tapa-lib/tapa/host/frt/c_api.h
```

## Implementation conventions

- Keep reusable logic in one typed home; do not copy adapters between crates.
- Alias colliding type names once at the consuming module, such as
  `tapa_ir::Port as IrPort` and `tapa_rtl::Port as RtlPort`.
- Use structured domain errors. Engines return errors; the CLI renders
  user-facing diagnostics.
- Keep unit tests next to implementation, functional behavior in
  `tests/functional/`, and larger design regressions in `tests/regression/`.
  Golden outputs are semantic references and require semantic review.
- Treat the roughly 800-line file budget and small-module goal as review
  guidance, not as a reason to add new abstractions.

## Public API surface audit

The development-branch CI runs `cargo public-api` for `tapa-protocol`,
`tapa-ir`, and `tapa-rtl`, then diffs the output against `docs/api/`. Revert
accidental changes or re-bless intentional API changes in the same pull
request.

Use the pinned toolchain from the CI workflow when regenerating a baseline:

```bash
rustup toolchain install nightly-2026-07-30 \
  --profile minimal --component rust-docs-json
cargo +nightly-2026-07-30 install cargo-public-api --locked --version 0.52.0

cd tapa-core
for crate in tapa-protocol tapa-ir tapa-rtl; do
  cargo +nightly-2026-07-30 public-api --package "$crate" \
    > "../docs/api/$crate.public-api.txt"
done
```
