# Provenance: `vadd` HLS-output fixtures

## What these are

Hand-authored stand-ins for the Vitis HLS output of every task of
`../vadd.cpp` (`Add`, `Mmap2Stream`, `Stream2Mmap`, and the `VecAdd` upper
shell), laid out as `<task>/{verilog,report}/` to mirror the
`<work_dir>/hls/` tree `tapa synth` harvests. `tapa_xo` targets with
`hls_fixtures` set stage these files and run `tapa synth
--skip-hls-based-on-mtime`, so analyze -> synth -> pack -> Verilator cosim
runs with no Xilinx tools at all; only TAPA's own analyze, codegen, pack,
and frt-cosim code is exercised.

## Interface sources

Module headers follow the conventions pinned by `tapa-protocol` (ap_ctrl
handshake, `{base}_s{suffix}` stream ports with the EOT bit at payload bit
32, `{port}_offset` direct mmap offsets, the 29-suffix compact M-AXI master
set) against the real task-graph argument names of `../vadd.cpp`
(`mmap`/`stream`/`a`/`b`/`c`/`n`), cross-checked byte-for-byte against the
headers real Vitis HLS 2025.2 emits for this kernel. `VecAdd.v` deliberately
has no `s_axi_control_*` ports: their absence is what makes codegen select
the HLS-flow top-level shape (`top_instantiates_control_s_axi == false`),
and its body stays empty because codegen discards upper-task bodies.

## Bodies

Lower-task bodies are behavioral and single-transaction-at-a-time
(single-beat M-AXI, one stream element in flight); they model kernel
semantics, not HLS microarchitecture. `Add.v` contains no FP arithmetic of
its own: it instantiates the same
`Add_fadd_32ns_32ns_32_7_full_dsp_1_ip` AXIS stub real HLS output uses, and
frt-cosim's Verilator runner replaces that stub with its DPI behavioral
model (`fp32_add`, latency 5 from the shipped `_ip.tcl`, a byte copy of what
Vitis HLS 2025.2 emits for this kernel).

The `*_csynth.xml` reports are minimal well-formed documents feeding
`report.json` metrics through the mtime-skip reload path; their numbers are
nominal, not measured.

## Regeneration

These files are maintained by hand. The contract they pin is the
task-RTL interface consumed by `tapa-codegen` and the cosim testbench; if a
change to `../vadd.cpp`'s task signatures or to the `tapa-protocol` naming
conventions breaks `vadd-fixture-zipsim`, update the headers here to match
the new contract (the real HLS headers of this kernel are the reference).
