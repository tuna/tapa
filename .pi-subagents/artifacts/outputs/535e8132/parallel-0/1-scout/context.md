# Code Context

## Files Retrieved
- `tapa-core/tapa-codegen/src/children.rs` (lines 246-276, 698-715, 477-486, 1110-1378): stream/peek port binding and reset routing paths include multiple legacy/fallback branches.
- `tapa-core/tapa-codegen/src/distributed_control.rs` (lines 35-47, 340-366, 698-763): distributed-control reset signal synthesis attributes and parse/validation of `_TO_` region tokens.
- `tapa-core/tapa-codegen/src/async_mmap.rs` (lines 36-40, 66-77, 229-244, 330-340, 474-491): async-mmap active tag probing and `_peek` suffix fallback/bridge wiring.
- `tapa-core/tapa-codegen/src/fifos.rs` (lines 343-396, 121-136, 149-191): FIFO width resolution with RTL/topology fallback and internal FIFO topology branch.
- `tapa-core/tapa-codegen/src/template.rs` (lines 42-72): stream port naming templates duplicated from child/fifo expectations.
- `tapa-core/tapa-codegen/src/lib.rs` (lines 149-170): reset declaration and cleanup logic that toggles floorplan/non-floorplan behavior.
- `tapa-core/tapa-codegen/src/axi_pipeline.rs` (lines 464-476): duplicate `parse_atomic_region` logic for `_TO_` slots.
- `tapa-core/tapa-codegen/src/generate_rtl_tests.rs` (lines 453-463, 2307-2310): explicit golden checks pinning legacy reset/peek behavior.
- `tapa-core/tapa-codegen/src/rtl_state.rs` (line 1647): test validating legacy direct-mmap canonical instance naming.
- `tapa-core/tapa-floorplan/src/partition/ilp.rs` (lines 47-49, 150-170, 901-903): legacy `lhs:rhs` compatibility for region-limit keys plus explicit compat entry points.
- `tapa-core/tapa-floorplan/src/partition/cut.rs` (lines 44-54): compatibility wrapper `find_cuts` for flat placement flow.
- `tapa-core/tapa-floorplan/src/device/model.rs` (lines 154-172, 175-180): region-name parsing helpers and constants.
- `tapa-core/tapa-floorplan/src/xdc.rs` (lines 109-139, 391-402): reset false-path constraints and separate region parsing helper.

## Key Code
- `distributed_control::fabric_reset_signal` (distributed_control.rs:37-47) branches on `distributed_control` to emit either a plain wire or `wire (* max_fanout = 256 *)`; this is the root of legacy/non-legacy RTL reset preservation.
- `children::build_child_instance`/`build_child_instance_with_reset` (children.rs:198-236, 698-751): per-child reset selection (`HANDSHAKE_RST_N` passthrough vs `DistributedControlPlan::child_reset_name`) and legacy fallback reset polarity (`.rst(ap_rst)`) when no distributed control.
- `children::stream_signal` and `resolve_peek_port_name` (children.rs:239-266, 475-486): resolve child stream ports via `get_port_of(... _s...)` / `_peek...` with fallback naming.
- `async_mmap::child_portargs` (async_mmap.rs:230-255): fallback search for `{prefix}_peek` for read-data/resp peeks.
- `fifos::resolve_fifo_width` (fifos.rs:343-395): 2-step width resolution: attached RTL first, then topology ports; else hard error.
- `fifos::build_internal_fifo_instance` (fifos.rs:129-135): floorplanned + depth-adjustment fallback to `build_registered_ready_fifo_instance` when depth widening required.
- `partition/ilp::lookup_limit` (ilp.rs:893-906): supports legacy `lhs:rhs` region specifier via `replace(':', "_TO_")`.
- `partition/cut::find_cuts` and `ilp::floorplan_flat`/`floorplan_multilevel` (cut.rs:44-54, ilp.rs:150-185): explicit “compatibility” entry points.
- Region parsing is duplicated: `parse_atomic_region` in distributed_control.rs:755-761, axi_pipeline.rs:464-470, and `xdc::parse_region_or_slot` lines 395-402, while model has `Coor::from_region_name`/`parse_slot_tag` (model.rs:167-172, 176-180).

## Architecture
- `tapa-codegen` generates top-level/task RTL in `lib.rs` via `generate_rtl`, which sets up reset synthesis attributes and then calls `children`/`fifos`/`async_mmap`/`distributed_control` generation.
- `children.rs` handles per-child instance wiring (stream/peek/mmap ports) and injects either distributed-local or legacy top-level reset into child instances and async-mmap bridges.
- `distributed_control.rs` defines distributed control control-plane objects and local/global control instances; its reset signal and max-fanout behavior is consumed by child/bridge code generation.
- `fifos.rs` owns internal/external FIFO instantiation and wire-width resolution from producer topology or RTL.
- `tapa-floorplan` computes floorplans and XDC. `partition/ilp.rs` and `partition/cut.rs` build scheduling/constraints, while `xdc.rs` emits region and reset timing constraints from results.

## Review findings
1. **HIGH – Legacy/fallback semantics are woven into active data-path code**
   - `children.rs:246-276` and `children.rs:698-715` keep dual stream naming (`_s`, `_peek`) and legacy parent-reset (`.rst(ap_rst)`) behavior in production logic. With no backward-compat requirement, this should be removed or isolated behind explicit feature gates to reduce complexity.

2. **HIGH – Legacy `_peek` fallback in async-mmap bridging is still active**
   - `async_mmap.rs:229-244` and `async_mmap.rs:474-490` explicitly search/build `{prefix}_peek` ports as compatibility fallback. This is a direct compat shim in generated module interfaces.

3. **MEDIUM – Reset distribution behavior is split across modules without a shared primitive**
   - `distributed_control.rs:37-47` (max-fanout wiring policy), `children.rs:698-715` (where reset source is chosen per child), `xdc.rs:129-137` (reset false-path constraints), and `xdc` string match pattern `*__tapa_control_fabric_reset_n*` are conceptually coupled but not centralized.

4. **MEDIUM – Legacy fifo-width fallback remains in width-resolution path**
   - `fifos.rs:343-395` falls back from producer RTL width to topology width when RTL is missing/incomplete. This compatibility fallback can mask bad interface contracts under strict forward-only assumptions.

5. **MEDIUM – Legacy region-limit syntax remains accepted in constraints**
   - `partition/ilp.rs:47-49` and `ilp.rs:901-903` still accept `lhs:rhs` region keys while canonical form is `_TO_`.

6. **MEDIUM – Duplicate region/slot parsing exists across codegen/floorplan**
   - `distributed_control.rs:755-761`, `axi_pipeline.rs:464-470`, `xdc.rs:395-402`, and `device/model.rs:167-180` reimplement `SLOT_X...Y...` token parsing instead of reusing one helper.

7. **LOW – Test-only scaffolding locks in removed compat semantics**
   - `generate_rtl_tests.rs:453-463` and `generate_rtl_tests.rs:2307-2310` are golden checks for legacy reset behavior.
   - `rtl_state.rs:1647` asserts legacy direct-mmap canonical instance naming.

8. **LOW – Duplicate constants/compat wrappers increase coupling in floorplan config**
   - `device/model.rs:32` and `partition/ilp.rs:30` both define `DEFAULT_USAGE_LIMIT = 0.7`; `ilp.rs:150-170` and `cut.rs:44-54` keep compatibility wrappers around old strategy choices.

## Start Here
- Start at `tapa-core/tapa-codegen/src/children.rs` around lines 198-276 and 698-715 to isolate/centralize legacy naming/reset fallbacks before propagating changes into `fifos.rs`, `async_mmap.rs`, and `distributed_control.rs`.
