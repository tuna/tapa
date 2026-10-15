# Full FPGA Compilation

Compile a TAPA design to an FPGA bitstream and run it on hardware.

## When to use this

Use this guide after software simulation passes (see [Your First Run](first-run.md))
and you are ready to target real hardware or run a more accurate RTL-level
simulation.

## What you need

- TAPA installed — see [Installation](installation.md)
- Xilinx Vitis 2022.1 or newer
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
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vecadd.xo
```

| Flag | Meaning |
|---|---|
| `--top` | Name of the top-level TAPA task |
| `--part-num` | Target FPGA part number |
| `--clock-period` | Target clock period in nanoseconds |
| `-f` | Kernel source file |
| `-o` | Output XO file |

```admonish note
You can replace `--part-num` and `--clock-period` with `--platform` to
target a Vitis platform directly, for example:

    --platform xilinx_u55c_gen3x16_xdma_3_202210_1

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

## V80 service-shell path

The Vitis `v++` and XRT stages below are for U-series Alveo cards. V80 uses
SLASH and VRT, so keep floorplanning in TAPA's explicit pipeline and use
`tapa link` for the final `.vbin`:

```bash
tapa -w work analyze --top Callipepla --input callipepla.cpp
tapa -w work synth --platform slash_v80_service --clock-period 10
tapa -w work floorplan --connectivity callipepla.cfg \
  --partition-strategy multi-level --pp-scheme double
tapa -w work pack --output callipepla.xo --connectivity callipepla.cfg
tapa -w work link --xo callipepla.xo --connectivity callipepla.cfg \
  --output callipepla.vbin --clock-hz 100000000 \
  --floorplan-xdc work/floorplan.xdc
```

This runs HLS, TAPA's SLR placement and crossing-pipeline insertion, then
SLASH/Vivado implementation. `tapa link` rejects an image that misses the
requested clock. Run the resulting `.vbin` through VRT; the [board guide](../howto/build-and-run.md)
shows the VRT host flags.

## Stage 3 — Link to xclbin

Use Vitis `v++` to link the `.xo` into a hardware bitstream. This step does
not involve TAPA and typically takes several hours:

```bash
v++ -o vadd.xilinx_u55c_gen3x16_xdma_3_202210_1.hw.xclbin \
  --link \
  --target hw \
  --kernel VecAdd \
  --platform xilinx_u55c_gen3x16_xdma_3_202210_1 \
  vecadd.xo
```

Artifact produced: `vadd.xilinx_u55c_gen3x16_xdma_3_202210_1.hw.xclbin`

```admonish note
`v++` accepts only platforms supported by the installed Vitis version. If
linking fails with `ERROR: [v++ 60-1299] The specified platform is not
supported`, choose a supported platform that matches both your board and Vitis
installation. See [Common Errors](../troubleshoot/common-errors.md).
```

```admonish warning
Hardware binary generation typically takes several hours. Plan accordingly,
and ensure your machine will remain available for the full duration.
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
