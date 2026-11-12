# Full FPGA Compilation

Compile a TAPA design to an FPGA bitstream and run it on hardware.

## When to use this

Use this guide after software simulation passes (see [Your First Run](first-run.md))
and you are ready to target real hardware or run a more accurate RTL-level
simulation.

## What you need

- TAPA installed — see [Installation](installation.md)
- Xilinx Vitis 2022.2 or newer
- A compatible Alveo platform (the examples below use the U55C)
- The vadd source files and a host executable built as described in
  [Your First Run](first-run.md)
- XRT for the on-board step

## Stage 1 — Synthesize to RTL

Run `tapa compile` to translate the C++ kernel into an RTL object (`.xo`):

```bash
tapa \
  compile \
  --top VecAdd \
  --platform xilinx_u55c_gen3x16_xdma_3_202210_1 \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vecadd.xo
```

| Flag | Meaning |
|---|---|
| `--top` | Name of the top-level TAPA task |
| `--platform` | Target Vitis platform; it supplies the FPGA part, and Stage 3 links against it |
| `--clock-period` | Target clock period in nanoseconds |
| `-f` | Kernel source file |
| `-o` | Output XO file |

```admonish note
Without the platform installed, `--part-num xcu55c-fsvh2892-2L-e` in place
of `--platform` still produces an `.xo` for Stage 2, but `tapa link` in Stage 3
needs the platform recorded here.

Per-task HLS reports are written to `work.out/hls/<TASK>/report/`, and a
combined timing and area summary to `work.out/report.json` and
`work.out/report.yaml`. See [Output Files](../reference/output-files.md).
```

Artifact produced: `vecadd.xo`

```admonish tip
For a supported multi-SLR design with cross-SLR timing or congestion issues,
run `tapa floorplan` and repack before cosimulation. `tapa compile` does not
include that step; see [Floorplan for Timing Closure](../howto/floorplan.md).
```

## Stage 2 — Fast hardware simulation

Before waiting hours for a full bitstream, validate the RTL with TAPA's
fast cosimulation. Pass the `.xo` file as the `--bitstream` argument:

```bash
# Rebuild the host only if you have not already done so in Your First Run.
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd
./vadd --bitstream=vecadd.xo 1000
```

Fast cosim uses simplified models for external components (DRAM, AXI
interconnect) so setup takes only a few seconds instead of the ten-plus
minutes that Vitis cosimulation requires. A successful run prints `PASS!`.

```admonish note
The default simulator backend is `xsim`, which requires Vivado on Linux. To use
Verilator instead (Linux or macOS, no Vivado required), pass
`-cosim_simulator verilator` to the host executable:
`./vadd --bitstream=vecadd.xo -cosim_simulator verilator 1000`.
That path needs Verilator 5.044 or newer, which is ahead of most distribution
packages — see [Fast Hardware Simulation](../howto/fast-cosim.md) before
reaching for it.
```

## V80 SLASH path

The Vitis `v++` and XRT stages below are for U-series Alveo cards. The V80
goes through SLASH instead: select the `xilinx-slash` target at analysis,
synthesize against the chosen shell, and link the `.zip` package with
`tapa link`:

```bash
tapa -w work analyze --top VecAdd --input vadd.cpp \
  --target xilinx-slash
tapa -w work synth --platform slash_v80_service
tapa -w work floorplan --connectivity v80.cfg
tapa -w work pack --output vecadd.zip
tapa -w work link --package vecadd.zip \
  --connectivity v80.cfg --floorplan-xdc work/floorplan.xdc \
  --output vecadd.vbin
```

`slash_v80_service` supplies part `xcv80-lsva4737-2MHP-e-S` and a 3.33 ns
(300 MHz) default clock. Use `slash_v80_compute` for the compute shell, with
the same 3.33 ns (300 MHz) default and `DDR0`–`DDR11` connectivity. This
runs HLS, optionally TAPA's SLR placement and crossing-pipeline insertion,
then SLASH/Vivado implementation. If routing misses the requested clock, the
link warns and records the achieved frequency in the vbin instead of
failing. Run the resulting `.vbin` through VRT with
`--vrt_bdf`; see [Alveo V80 with SLASH](../howto/v80.md) for prerequisites
and the full flow.

## Stage 3 — Link to xclbin

Link the `.xo` into a hardware bitstream with `tapa link`, which drives
`v++ --link` using the kernel name and platform recorded in the package:

```bash
tapa link \
  --package vecadd.xo \
  -o vadd.xilinx_u55c_gen3x16_xdma_3_202210_1.hw.xclbin
```

Artifact produced: `vadd.xilinx_u55c_gen3x16_xdma_3_202210_1.hw.xclbin`

```admonish note
`tapa link` builds a PL kernel's hardware bitstream only, which typically
takes several hours. For a Vitis hardware-emulation build
(`v++ --target hw_emu`), invoke `v++` directly as shown in
[Vitis Cosimulation](../howto/vitis-cosim.md). A top on the AI Engine with
no PL kernel is the exception: `tapa link --sim` builds its hardware
emulation (see [AI Engine Tasks](../howto/aie.md#a-top-with-no-pl-kernel)).
If linking fails with `[v++ 60-1299]`, the platform is not supported by your
Vitis version — see [Common Errors](../troubleshoot/common-errors.md).
```

## Stage 4 — On-board execution

With an Alveo card installed and XRT configured, run the host binary and
point it at the generated xclbin:

```bash
./vadd --bitstream=vadd.xilinx_u55c_gen3x16_xdma_3_202210_1.hw.xclbin
```

A successful on-board run prints `PASS!`, confirming the accelerator
produced correct results on real hardware.

## Next step

[The Programming Model](../concepts/programming-model.md)
