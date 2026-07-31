# Provenance: `vadd` golden case

## Design

`design.json` is a byte copy of `testdata/topology/vadd_design.json` — the
same post-synthesis `tapa-ir` design fixture `tapa-ir`'s
`design_conformance.rs` consumes — frozen here so the golden inputs are
self-contained (`VecAdd` upper task; `Add`, `Mmap2Stream` x2, `Stream2Mmap`
lower tasks; three internal FIFOs; three mmap ports). The shared fixture is
left untouched; both conformance tests keep consuming it.

## HLS inputs

`inputs/*.v` are hand-authored, **interface-faithful minimized Vitis-HLS
fixtures**, one per HLS task, standing in for what `tapa synth`'s HLS step
would emit for `tests/apps/vadd/vadd.cpp` (running real Vitis HLS is not
part of the unit-test environment). Port spellings follow the conventions
pinned by `tapa-protocol` and observed in real HLS output
(`tapa-xilinx/testdata/xilinx/real/vadd.v`, `testdata/rtl/UpperLevelTask.v`):

- ap_ctrl handshake (`ap_clk`, `ap_rst_n`, `ap_start`, `ap_done`,
  `ap_idle`, `ap_ready`) on every task;
- stream payloads one bit wider than the element type (EOT bit):
  `[32:0]` for `float` streams; consumer streams carry the
  `*_peek_dout` / `*_peek_empty_n` / `*_peek_read` trio;
- mmap arguments realize the full compact M-AXI master set (29 suffixes,
  64-bit address, 32-bit data, 1-bit ID) plus `{port}_offset`;
- the upper task additionally carries the 16 `s_axi_control_*` ports,
  `interrupt`, and the `C_S_AXI_CONTROL_*` parameters; M-AXI masters are
  **not** in the fixture — codegen adds them, so their presence in the
  blessed output is part of what is frozen.

Bodies are empty (`endmodule`), matching how codegen consumes the fixtures:
upper-task bodies are discarded (`body_text.clear()`), and lower tasks are
never re-emitted (the CLI ships their original files verbatim — only the
generated RTL and templates are blessed here; the embedded support
assets are blessed once, repo-wide, under `_assets/`).

## Blessed output

`expected/` mirrors the pack-step tree: `rtl/` = `generate_rtl`'s
`generated_files` + every embedded `VerilogAssets` support file;
`template/` would hold custom-RTL templates (this case emits none). Content
is normalized (trailing whitespace trimmed, single trailing newline).
Regenerate with `TAPA_BLESS_GOLDEN=1 cargo test -p tapa-codegen --test
golden_rtl`.
