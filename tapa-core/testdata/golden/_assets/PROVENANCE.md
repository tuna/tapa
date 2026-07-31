# `_assets` — shared support-asset pin

The embedded support assets (`tapa-codegen/assets/verilog/`, rust-embed'd
into the binary) are case-invariant: every golden case's shipped tree
contains exactly this set, written by the CLI's
`write_verilog_support_assets` loop (`generate_rtl` deliberately does not
return the assets — the F1 drift seam tracked in the refactor plan).

They are blessed once here rather than identically in every case dir, so
an asset edit costs one re-bless hunk instead of one per case. The harness
(`golden_rtl.rs`) replays the CLI's asset write against this tree the same
way it compares generated RTL per case.
