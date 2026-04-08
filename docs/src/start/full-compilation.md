# Full FPGA Compilation

Compile a TAPA design to an FPGA bitstream and run it on hardware.

## When to use this

Use this guide after software simulation passes (see [Your First Run](first-run.md))
and you are ready to target real hardware or run a more accurate RTL-level
simulation.

## What you need

- TAPA installed — see [Installation](installation.md)
- Xilinx Vitis 2022.1 or newer
- A compatible Alveo platform (the examples below use the U250)
- The vadd source files: [`vadd.cpp`](https://github.com/tuna/tapa/blob/main/tests/apps/vadd/vadd.cpp) and [`vadd-host.cpp`](https://github.com/tuna/tapa/blob/main/tests/apps/vadd/vadd-host.cpp)

## Stage 1 — Synthesize to RTL

Run `tapa compile` to translate the C++ kernel into an RTL object (`.xo`):

```bash
tapa \
  compile \
  --top VecAdd \
  --part-num xcu250-figd2104-2L-e \
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

    --platform xilinx_u250_gen3x16_xdma_4_1_202210_1

HLS reports are written to `work.out/report/` after synthesis completes.
```

Artifact produced: `vecadd.xo`

## Stage 2 — Fast hardware simulation

Before waiting hours for a full bitstream, validate the RTL with TAPA's
fast cosimulation. Pass the `.xo` file as the `--bitstream` argument:

```bash
./vadd --bitstream=vecadd.xo 1000
```

Fast cosim uses simplified models for external components (DRAM, AXI
interconnect) so setup takes only a few seconds instead of the ten-plus
minutes that Vitis cosimulation requires. A successful run prints `PASS!`.

```admonish note
The default simulator backend is `xsim`, which requires Vivado on Linux. To use
Verilator instead (cross-platform, no Vivado required), pass `-cosim_simulator verilator`
to the host executable: `./vadd --bitstream=vadd.xo -cosim_simulator verilator`.
```

## Stage 3 — Link to xclbin

Use Vitis `v++` to link the `.xo` into a hardware bitstream. This step does
not involve TAPA and typically takes several hours:

```bash
v++ -o vadd.xilinx_u250_gen3x16_xdma_4_1_202210_1.hw.xclbin \
  --link \
  --target hw \
  --kernel VecAdd \
  --platform xilinx_u250_gen3x16_xdma_4_1_202210_1 \
  vecadd.xo
```

Artifact produced: `vadd.xilinx_u250_gen3x16_xdma_4_1_202210_1.hw.xclbin`

```admonish warning
Hardware binary generation typically takes several hours. Plan accordingly,
and ensure your machine will remain available for the full duration.
```

## Stage 4 — On-board execution

With an Alveo card installed and XRT configured, run the host binary and
point it at the generated xclbin:

```bash
./vadd --bitstream=vadd.xilinx_u250_gen3x16_xdma_4_1_202210_1.hw.xclbin
```

A successful on-board run prints `PASS!`, confirming the accelerator
produced correct results on real hardware.

## Next step

[The Programming Model](../concepts/programming-model.md)
