# Provenance: `async-mmap` golden case

## Design: adapted topology (documented fallback)

The preferred route — a real `tapacc` design fixture for
`tests/apps/async_mmap/async_mmap.cpp` — was evaluated and rejected for
this environment: producing it needs the Clang-based `tapacc` binary,
which is neither prebuilt here nor buildable within the effort budget
(the Bazel `tapacc_conformance_test` path exists precisely because a real
`tapacc` invocation is this heavy).

`design.json` is therefore hand-adapted from the `vadd` case's topology to
match the exact kernel `tests/apps/async_mmap/async_mmap.cpp` (the same
file the tapacc conformance suite analyzes): top task `AsyncTop` with a
plain `mmap` port `mem`, scalar `n`, and an external `ostream` `data_q`; one
child `AsyncReader` whose `async_mmap` port `mem` binds to the top's
`mem`. The external stream is modeled as a depth-less FIFO with only
`produced_by`, per `tapa-ir`'s `InterconnectDefinition` schema, and the
selected flow is `xilinx-hls`, matching the conformance invocation. When a
real tapacc fixture for this kernel is ever captured, swapping it in and
re-blessing (`TAPA_BLESS_GOLDEN=1`) is the intended upgrade path.

## HLS inputs

Hand-authored, interface-faithful fixtures in the same style as the `vadd`
case. `AsyncReader.v` activates only the read half of the async-mmap
channel set (`mem_read_addr_*` producer + `mem_read_data_*` consumer with
the peek trio) and ties the unused write-side activity outputs to a
constant zero — the pattern `tapa-codegen::async_mmap` sniffs to prune a
bridge direction, so the blessed bridge carries
`EnableReadChannel(1)` / `EnableWriteChannel(0)`. `AsyncTop.v` is the
`vadd` top shape plus the external stream ports (`out_din`, `out_full_n`,
`out_write`).

## Blessed output

Same rules as the other cases (`rtl/` = generated RTL, support assets
under `_assets/`, normalized). This case exercises: the FIFO-style async-mmap bridge
insertion (`mem__m_axi`), read-only bridge parameter pruning, the
per-instance `*_offset` pipeline, external-stream passthrough, and the
monolithic-FSM control path.
