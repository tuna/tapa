# Code Context

## Files Retrieved
1. `tapacc/codegen/schema_fields.h` (lines 1-44) — central C++ schema key constants used by `tapacc` JSON emitter.
2. `tapacc/tapacc.cpp` (lines 139-170) — serialization site that emits all root/task/port fields consumed by Rust `tapa-ir`.
3. `tapa-core/tapa-ir/src/task.rs` (1-80), `tapa-core/tapa-ir/src/port.rs` (1-80), `tapa-core/tapa-ir/src/task.rs` (17-24), `tapa-core/tapa-ir/src/target.rs` (8-35) — Rust task-graph model/serde contract.
4. `tapacc/codegen/conventions.h` (9-43) — naming helper constants for ABI-sensitive rewrite names.
5. `tapacc/codegen/xilinx.cpp` (239-270) and `tapa-core/tapa-codegen/src/children.rs` (239-276, 380-423) — stream/mmap naming and fallback behavior.
6. `tapa-core/tapa-codegen/src/template.rs` (52-112) — stream and mmap port-name/width synthesis.
7. `tapacc/frontend/ports.cpp` (19-104) and `tapacc/frontend/BUILD.bazel` (48-55, 60-82, 148-161, 177-186) — frontend port width extraction and `tapa_stub_decls` consumers.
8. `tapa-lib/tapa.h`, `tapa-lib/tapa/compat.h` (21-26, 14-16) and `tapacc/codegen/xilinx.cpp:269` — explicit compatibility language/aliases.

## Findings

1. **Stale comment references removed legacy API** (low)
- **file:line:** `tapacc/codegen/xilinx.cpp:269`
- **description:** The comment says “Mirrors the legacy rewriter’s RewriteTopLevelFuncArguments,” but `RewriteTopLevelFuncArguments` is no longer present in this C++ tree (this token appears only in that comment), leaving stale design history embedded in active rewrite logic and increasing ambiguity for future refactors.

2. **Cross-language single-source-of-truth gap for ABI naming/width** (medium)
- **file:line:** `tapacc/codegen/conventions.h:13-22`, `tapacc/frontend/ports.cpp:56-73`, `tapa-core/tapa-codegen/src/template.rs:52-71`, `tapa-core/tapa-codegen/src/children.rs:239-277`
- **description:** Tapacc canonicalizes names like `_offset` and array offsets in one place and computes raw `width` in frontend ports, while Rust codegen independently hardcodes stream signal names (`_s_*`, `_peek*`) and stream width inflation (`saturating_add(1)`) in separate modules; there is no shared constants module to keep these ABI surface values synchronized.

3. **Compatibility/fallback paths still present across active C++/Rust surfaces** (medium)
- **file:line:** `tapa-lib/tapa.h:21-26`, `tapa-lib/tapa/compat.h:14-16`, `tapa-core/tapa-codegen/src/children.rs:258-277,380-387`
- **description:** `tapa::hls::stream` is still a compatibility alias in user headers, and codegen keeps multiple fallback probes for stream/mmap port naming variants, so compatibility/legacy behavior is still part of runtime behavior rather than a deleted legacy-only path.

4. **`tapa_stub_decls.h` is test-only and currently used** (low)
- **file:line:** `tapacc/frontend/BUILD.bazel:48-55`, `tapacc/frontend/BUILD.bazel:60-82,148-161`, `tapacc/codegen/BUILD.bazel:134-144`
- **description:** `tapa_stub_decls` is marked `testonly = True` and referenced by many frontend/codegen tests, so it is not dead but explicitly scoped scaffold for tests.

## Key Code
- `tapacc/codegen/schema_fields.h` defines constants like `kFieldTop`, `kFieldTarget`, `kFieldTasks`, `kFieldTop`, etc.; `tapacc/tapacc.cpp` serializes JSON with the same keys.
- `tapa-core/tapa-ir/src/task.rs` and `port.rs` define the mirrored contract with serde/`deny_unknown_fields`, and `ArgCategory::Mmap` intentionally aliases `hmap`.
- `tapacc/frontend/ports.cpp` sets raw `elem_width` and channel metadata in the emitted graph; `tapa-core/tapa-codegen/src/template.rs` adds stream-level width inflation when building RTL shell interfaces.
- `tapacc/codegen/xilinx.cpp` rewrites signatures and appends compatibility-focused comments; `tapa-core/tapa-codegen/src/children.rs` contains stream/mmap fallback logic for legacy port spelling.

## Architecture
- **Tapacc frontend (`tapacc/frontend/*`)** scans C++ and builds a typed model.
- **Tapacc emitter (`tapacc/codegen/*`)** rewrites signatures/streams and emits the final JSON schema and code.
- **Rust analyzer model (`tapa-core/tapa-ir`)** deserializes and validates the schema with strict serde fields.
- **Rust codegen (`tapa-core/tapa-codegen`)** generates RTL interfaces/modules from the model and resolves stream/mmap hookups with compatibility fallbacks.

## Start Here
Open `tapacc/tapacc.cpp` (schema emission), then follow into `tapa-core/tapa-ir/src/task.rs` and `port.rs` for consumer contract, then `tapa-core/tapa-codegen/src/template.rs` and `children.rs` for the current stream/mmap naming/width fallbacks.

```json
{
  "findings": [
    {
      "id": "F-1",
      "severity": "low",
      "file": "tapacc/codegen/xilinx.cpp",
      "line": 269,
      "summary": "Stale comment references removed legacy rewriter symbol `RewriteTopLevelFuncArguments`.",
      "evidence": [
        "grep for RewriteTopLevelFuncArguments in tapacc returns only this comment occurrence"
      ]
    },
    {
      "id": "F-2",
      "severity": "medium",
      "file": "tapacc/codegen/conventions.h",
      "line": 13,
      "summary": "Port name/width conventions span C++ and Rust in separate files with no shared constants, creating schema drift risk.",
      "evidence": [
        "C++ emits raw `width` in frontend ports and offset names via conventions helpers",
        "Rust template emits `{name}_s_dout/_s_din/_peek*` with `+1` width and children.rs fallback probes"
      ]
    },
    {
      "id": "F-3",
      "severity": "medium",
      "file": "tapa-lib/tapa.h",
      "line": 21,
      "summary": "Active compatibility alias and fallback behaviors remain in production and codegen paths.",
      "evidence": [
        "tapa.h documents compatibility layer",
        "compat.h aliases `tapa::hls::stream` to infinite-depth stream wrapper",
        "children.rs resolves alternate stream/mmap port spellings conditionally"
      ]
    },
    {
      "id": "F-4",
      "severity": "low",
      "file": "tapacc/frontend/tapa_stub_decls.h",
      "line": 1,
      "summary": "Not dead code: test-only header is actively used by multiple frontend/codegen tests.",
      "evidence": [
        "BUILD marks target as testonly and codegen/frontend tests depend on it"
      ]
    }
  ]
}
```
