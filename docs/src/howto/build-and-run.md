# Build & Run on Board

**Purpose:** Build a TAPA design into an FPGA bitstream and run it on an Alveo board.

**When to use this:** After fast cosim (and optionally Vitis cosim) passes — this step converts your `.xo` kernel object into a hardware bitstream and executes it on real silicon.

## What you need

- A `.xo` kernel object from `tapa compile` and its matching host executable
- Vitis and XRT installed (Linux only)
- The target platform string (e.g., `xilinx_u55c_gen3x16_xdma_3_202210_1`)
- An Alveo board installed in the system for the final execution step
- Several hours of compute time for full implementation

For the V80 SLASH service shell, `tapa link` produces a `.vbin` that the
host program runs through VRT with `--vrt_bdf`; see
[Alveo V80 with SLASH](v80.md) for that flow. The XRT flow below produces an
`.xclbin` for U-series devices.

## Stage 1: Compile the kernel with TAPA

If you do not already have a `.xo`, produce it with `tapa compile`:

```bash
platform=xilinx_u55c_gen3x16_xdma_3_202210_1

tapa \
  --work-dir work.out \
  compile \
  --top VecAdd \
  --platform "$platform" \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vadd.$platform.hw.xo
```

The `.xo` file feeds `tapa link`, which drives `v++ --link`.

## Stage 2: Link into an FPGA bitstream

```bash
tapa \
  --work-dir work.out \
  link \
  --package vadd.$platform.hw.xo \
  -o vadd.$platform.hw.xclbin
```

`tapa link` runs `v++ --link` on the tool host with the kernel name and
platform recorded inside the package, so they cannot drift from what
`tapa compile` targeted. Pass `--connectivity` for a design with direct M-AXI
ports, and `--floorplan-xdc work.out/floorplan.xdc` when you floorplanned;
`--clock-mhz` overrides the synthesized clock.

```admonish warning
This step takes **several hours** depending on design complexity and host machine performance. Plan accordingly and consider running it on a dedicated build server (see [Remote Execution](remote-execution.md)).
```

The output artifact is `vadd.$platform.hw.xclbin` — this is the bitstream loaded onto the FPGA.

Key alignment rules:
- The input package (`vadd.$platform.hw.xo`) must be the file produced by
  `tapa compile`; the link reads its target and kernel metadata from it.
- The output extension selects the backend's artifact: `.xclbin` for the
  `xilinx-vitis` target, `.vbin` for `xilinx-slash`.

## Stage 3: Execute on the FPGA

The same host executable used for software and hardware simulation runs on board:

```bash
./vadd --bitstream=vadd.$platform.hw.xclbin
```

## Expected output

```
INFO: Found platform: Xilinx
INFO: Found device: xilinx_u55c_gen3x16_xdma_3_202210_1
...
kernel time: 0.00042 s
PASS!
```

XRT log messages and kernel time vary by platform, design, and workload.

## Validation

The run is correct when:

1. XRT finds and selects the expected device.
2. The host process exits successfully.
3. The application's correctness check prints `PASS!`.

```admonish tip
If you use `std::vector` for memory-mapped buffers, XRT may warn about
unaligned host pointers and make an extra copy. Use
`std::vector<T, tapa::aligned_allocator<T>>` to provide page-aligned storage
and avoid that alignment-related copy.
```

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for diagnosis steps. Common issues include XRT not finding the device, platform string mismatches, and bitstream generated for a different platform than the installed board.
```

---

**Next step:** [Remote Execution](remote-execution.md)
