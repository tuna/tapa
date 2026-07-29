# Code Context

## Files Retrieved
1. `tapa-core/tapa-rtl/src/module.rs` (lines 33-241) — port-name resolution, FIFO infix handling, singleton-array fallback, and tests exercising legacy naming fallbacks.
2. `tapa-core/tapa-rtl/src/parser/mod.rs` (lines 100-140, 435-448) — attribute parser fallback chain (structured -> raw) and module leading-attribute recovery.
3. `tapa-core/tapa-rtl/src/parser/tree_sitter.rs` (lines 100-124, 327-348, 310-317) — attribute wrapper parsing and comments about parser-era behavior.
4. `tapa-core/tapa-ir/src/instance.rs` (lines 34-41) — legacy canonical instance naming logic.
5. `tapa-core/tapa-ir/src/transforms.rs` (lines 185-187) — call site of `canonical_name` in flattening.
6. `tapa-core/tapa-protocol/src/lib.rs` (lines 8-19, 23-35) — handshake/reset and stream suffix naming constants.
7. `tapa-core/tapa-rtl/src/mutation.rs` (lines 21-45) — cleanup regexes with hard-coded control/FSM naming.

## Key Code
- `tapa-rtl/src/module.rs:64-90` (`VerilogModule::get_port_of`, `FIFO_INFIXES`, `match_array_name`)
- `tapa-rtl/src/module.rs:196-206` (V infix preference)
- `tapa-rtl/src/parser/mod.rs:100-115, 127-139` (`raw_attribute`, fallback in `attributes`)
- `tapa-rtl/src/parser/mod.rs:435-447` (module-leading attribute structured/raw fallback)
- `tapa-ir/src/instance.rs:35-41` (`TaskInstance::canonical_name`)
- `tapa-ir/src/transforms.rs:185` (`inst.canonical_name(child_def_name, idx)`)
- `tapa-protocol/src/lib.rs:8-19, 23-35` (protocol naming constants)
- `tapa-rtl/src/mutation.rs:39-45` (`AP_HANDSHAKE_ASSIGN_PATTERN`, cleanup regex cluster)

## Architecture
- `tapa-rtl` parses and emits Verilog (`parser`, `module`, `mutation`) and depends on `tapa-protocol` constants.
- `tapa-ir` owns task graph schema and flattening transforms; it emits instance names via `TaskInstance::canonical_name`.
- `tapa-protocol` is the naming/contract constants crate for handshake, stream, FIFO, and AXI naming conventions.

## Findings
1. **High** — Backward-compat singleton-array port fallback remains in RTL port lookup.
   - File/line: `tapa-rtl/src/module.rs:64-90` (and tests `219-234`)
   - `get_port_of` first tries sanitized+infix names then applies a special singleton fallback (`x[0]` -> `x_s...`) when no exact match exists.
   - This is explicit compatibility behavior and unnecessary if older Vitis array-port naming is no longer supported.

2. **High** — Raw-attribute parser fallback path is still present in attribute handling.
   - File/line: `tapa-rtl/src/parser/mod.rs:100-115, 127-139, 435-447`
   - Attribute parsing first attempts structured parse (`(* key = "value" *)`), then raw fallback (`(* ... *)`) and then a permissive skip path.
   - This preserves parsing across non-standard attribute forms and is a backward-compatibility shim.

3. **High** — Legacy canonical instance-name fallback is still used by flattening.
   - File/line: `tapa-ir/src/instance.rs:35-41` (called at `tapa-irt/src/transforms.rs:185-187`)
   - `canonical_name` emits `{definition}_{index}` when no explicit instance name exists.
   - If unnamed instances are impossible in the new flow, this fallback can be removed to simplify naming.

4. **Medium** — Protocol naming constants are not consistently the sole source of truth.
   - File/line: `tapa-protocol/src/lib.rs:8-19, 23-35` vs `tapa-rtl/src/mutation.rs:39-45`
   - Protocol defines shared handshake names (`ap_done`, `ap_ready`, etc.), but RTL cleanup regexes still hard-code subsets (`ap_(?:done|ready|idle)`) and a separate legacy FSM/removal set (`ap_*fsm`, `ap_ce_reg`).
   - This duplication risks drift between protocol contract and RTL cleanup behavior.

5. **Medium** — FIFO infix ordering/handling is local-only and can drift.
   - File/line: `tapa-rtl/src/module.rs:33-35`, `64-71`
   - `FIFO_INFIXES` (`_V`, `_r`, `_s`, ``) and matching order are encoded only in RTL, while protocol already advertises naming contracts.
   - New HDL pipeline should centralize this with protocol/shared constants to avoid hidden divergence.

6. **Low** — Potential stale wording about “old parser” in comments.
   - File/line: `tapa-rtl/src/parser/tree_sitter.rs:327-348`
   - Comments describe errors as skipped by the “old parser,” but current flow is a mixed tree-sitter + nom pipeline and terminology may no longer reflect architecture.

7. **No definitive dead/unused code found**
   - File/line: `tapa-core/tapa-rtl`, `tapa-core/tapa-ir`, `tapa-core/tapa-protocol` (full scan for `#[cfg(test)]`, `legacy`, commented-out code)
   - `cargo check` for each crate succeeded with no dead-code-related warnings emitted; no clear unused pub items or commented-out runtime code blocks were identified beyond docs/tests.

## Start Here
Open `tapa-core/tapa-rtl/src/module.rs` first; it contains the highest-density of compatibility/fallback logic (`FIFO_INFIXES`, `get_port_of`) and ties directly into protocol naming consistency and downstream parsing/mutation behavior.

## Supervisor coordination
Progress updated at `/home/dotkrnl/tapa/.pi-subagents/artifacts/progress/535e8132/progress.md`.