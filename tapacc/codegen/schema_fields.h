#pragma once
// Schema field-name constants for the tapa-ir task-graph JSON.
//
// These names are the wire contract between tapacc (this producer) and
// tapa-ir's serde types (the consumer). The conformance test
// (//tapa-core:tapacc_conformance_test) validates the full schema
// end-to-end; this header just centralizes the keys so a typo on either
// side is a build error (unknown constant) or a test failure (drifted
// value), never a silent key mismatch.

namespace tapa::cc {

// ── Root ────────────────────────────────────────────────────────────
inline constexpr const char* kFieldSchemaVersion = "schema_version";
inline constexpr const char* kFieldTop = "top";
inline constexpr const char* kFieldTarget = "target";
inline constexpr const char* kFieldTasks = "tasks";

// The schema version stamped on emitted task graphs. Must match
// `tapa_ir::SCHEMA_VERSION`; the conformance test locks the pair, and
// tapa-ir rejects graphs newer than it understands with a regenerate
// message instead of a field-level misparse.
inline constexpr int kSchemaVersion = 3;

// ── Task ────────────────────────────────────────────────────────────
// The per-task source manifest (schema v3). `srcs` names rewritten-tree
// files relative to the `-emit-dir` tapacc writes them into;
// `include_dirs` and `defines` are tree-relative include paths and the
// task's guard macros, both empty until the rewritten tree lands.
inline constexpr const char* kFieldSrcs = "srcs";
inline constexpr const char* kFieldIncludeDirs = "include_dirs";
inline constexpr const char* kFieldDefines = "defines";
inline constexpr const char* kFieldLevel = "level";
inline constexpr const char* kFieldSynth = "synth";
inline constexpr const char* kFieldReadableName = "readable_name";
inline constexpr const char* kFieldPorts = "ports";
inline constexpr const char* kFieldFifos = "fifos";

// ── Port ────────────────────────────────────────────────────────────
inline constexpr const char* kFieldCat = "cat";
inline constexpr const char* kFieldName = "name";
inline constexpr const char* kFieldType = "type";
inline constexpr const char* kFieldWidth = "width";
inline constexpr const char* kFieldChanCount = "chan_count";
inline constexpr const char* kFieldChanSize = "chan_size";

// ── Instance ────────────────────────────────────────────────────────
inline constexpr const char* kFieldStep = "step";
inline constexpr const char* kFieldArgs = "args";
inline constexpr const char* kFieldArg = "arg";
inline constexpr const char* kFieldValue = "value";

// ── FIFO / interconnect ─────────────────────────────────────────────
inline constexpr const char* kFieldDepth = "depth";
inline constexpr const char* kFieldProducedBy = "produced_by";
inline constexpr const char* kFieldConsumedBy = "consumed_by";

}  // namespace tapa::cc
