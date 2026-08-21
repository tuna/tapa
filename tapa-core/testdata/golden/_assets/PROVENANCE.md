# `_assets` — shared support-asset pin

The embedded support assets (`tapa-codegen/assets/verilog/`, rust-embed'd
into the binary) are case-invariant: every golden case's shipped tree
contains exactly this set. `generate_rtl` returns them inside its
`ArtifactManifest` (the F1 drift seam from the refactor plan is closed),
so the harness pins the manifest's support-asset slice
(`support_asset_files()`) — any case's manifest serves, since the slice is
identical for every design.

They are blessed once here rather than identically in every case dir, so
an asset edit costs one re-bless hunk instead of one per case. The harness
(`golden_rtl.rs`) compares this tree against that slice the same way it
compares each case's design-specific manifest slice against the case's own
blessed tree.

## Regeneration review (2026-08-21)

Twelve expected RTL files lost their three-line RapidStream copyright
header and nothing else (`160a4493`). AGENTS.md forbids carrying such a
header "on any file, new or existing", so this is policy, not drift. The
Verilog itself is byte-identical.
