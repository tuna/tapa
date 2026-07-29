# Code Context

## Files Retrieved
1. `tapa-core/tapa-cli/src/main.rs` (1-59) — program entrypoint and step execution bootstrap.
2. `tapa-core/tapa-cli/src/chain.rs` (19-188) — chained subcommand parsing/execution model.
3. `tapa-core/tapa-cli/src/globals.rs` (40-92) — global flags + root CLI parser.
4. `tapa-core/tapa-cli/src/steps/synth/mod.rs` (60-103) — `synth` CLI flags and compat surface.
5. `tapa-core/tapa-cli/src/steps/synth/runner.rs` (26-64, 80-103) — maps CLI args to HLS run options.
6. `tapa-core/tapa-cli/src/steps/synth/hls_run.rs` (130-192) — HLS scheduling, skip-cache logic, and `--keep-hls-work-dir` behavior.
7. `tapa-core/tapa-cli/src/steps/analyze/mod.rs` (102-115) — `--tapa-cpp` alias and deprecation context.
8. `tapa-core/tapa-cli/src/tapacc/discover.rs` (16-47, 128-169) — resource fallback paths and clang-family version verification.
9. `tapa-core/tapa-cli/src/tapacc/cflags.rs` (168-216) — GCC include-version probing for tool compatibility.
10. `tapa-core/tapa-cli/src/steps/pack/mod.rs` (116-134, 272-315) — per-task report directories and report-archive paths.
11. `tapa-core/tapa-cli/src/steps/pack/vitis_packaging.rs` (272-315) — report path collection and task-name namespacing.
12. `tapa-core/tapa-wrapper.sh` (8-44) — runfiles bootstrap + binary lookup fallback chain.
13. `tapa-core/tapa-xilinx/src/tools/hls/mod.rs` (345-360) — csynth filename fallback parsing.
14. `tapa-core/tapa-xilinx/src/tools/vitis/timing.rs` (57-93, 24-30) — clock-name + frequency-based parsing compatibility.
15. `tapa-core/tapa-xilinx/src/floorplan/implementation.rs` (21-23) — legacy floorplan artifact names.
16. `tapa-core/tapa-xilinx/src/runtime/process.rs` (88-123) — tool settings-root/env resolution and `settings64.sh` fallbacks.

## Key Code

### Finding 1 (MEDIUM): Parsed CLI flags are ignored (dead options)
- `tapa-cli/src/steps/synth/mod.rs:77-90, 98-100` define `--remove-hls-work-dir`, `--no-skip-hls-based-on-mtime`, and `--disable-synth-util`, but these fields are never read downstream.
- Evidence: only `keep_hls_work_dir` and `skip_hls_based_on_mtime` are propagated.
  - `runner.rs:57-60` maps only `skip_hls_based_on_mtime` and `keep_hls_work_dir`.
  - `hls_run.rs:157-169` controls project persistence only via `options.keep_work_dir`.
- Impact: `--remove-hls-work-dir`, `--no-skip-hls-based-on-mtime`, and `--disable-synth-util` are effectively no-ops (silent behavior divergence).

### Finding 2 (LOW): Unused compatibility toggle compounds the dead-flag issue
- `tapa-cli/src/steps/synth/mod.rs:80` documents `--remove-hls-work-dir` as a user-facing toggle, conflicting with keep.
- `steps/synth/hls_run.rs:177-179` explicitly states no explicit cleanup in cleanup logic; there is no branch that implements a dedicated "remove" mode.
- Combined with Finding 1, this indicates either legacy leftovers or incomplete implementation.

### Compatibility/behavior snapshots (non-blocking)
- `discover.rs:16-47` has explicit resource fallback chain for Bazel/source-tree variants and `discover.rs:140-143` verifies version output with a permissive `version (\d+)(\.\d+)*` regex.
- `tapacc/cflags.rs:183-216` selects latest compatible GCC by parsing semver directories and taking max version.
- `tools/hls/mod.rs:345-360` tries `{task}_csynth.xml` then `{task}.csynth.xml`.
- `tools/vitis/timing.rs:57-93` adds frequency-based platform-agnostic clock matching, falling back to historical `ap_clk`.
- `tapa-wrapper.sh:19-23` and `30-44` are multi-layout runfiles/manifest fallbacks for locating the Rust binary and anchoring sibling-tool discovery.

## Architecture
- Entrypoint (`main.rs`) parses CLI (`globals + Step`), sets work/temp env, bootstraps remote config, then dispatches `Step::execute`.
- `chain.rs` parses chained subcommands and recursively reparses trailing args via `ChainParser`.
- `synth` flow: `runner.rs` builds `HlsRunOptions` from CLI and calls `run_hls_for_leaves`.
- `hls_run.rs` builds per-task HLS plans and either cache-hits existing Verilog/csynth (`hdl_dir` mtimes + report parse) or executes tool invocations through `tapa-xilinx`.
- `tapa-xilinx` tool layer handles actual HLS/Vitis invocations and parsing:
  - HLS report naming compatibility, timing parser fallbacks, and `settings64.sh` sourcing.
- `tapacc/discover.rs` and `tapa-wrapper.sh` stabilize resource and tool discovery across source-tree/runfiles layouts.

## Start Here
- Open first: `tapa-core/tapa-cli/src/steps/synth/mod.rs` + `tapa-core/tapa-cli/src/steps/synth/runner.rs` + `tapa-core/tapa-cli/src/steps/synth/hls_run.rs` to address the dead/no-op CLI flags and confirm intended behavior.