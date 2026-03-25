# Build & Run on Board

**Purpose:** Build a TAPA design into an FPGA bitstream and run it on an Alveo board.

**When to use this:** After fast cosim (and optionally Vitis cosim) passes — this step converts your `.xo` kernel object into a hardware bitstream and executes it on real silicon.

## What you need

- A `.xo` kernel object from `tapa compile`
- Vitis and XRT installed (Linux only)
- The target platform string (e.g., `xilinx_u280_xdma_201920_3`)
- An Alveo board installed in the system for the final execution step
- Several hours of compute time for `v++ --link`

## Stage 1: Compile the kernel with TAPA

If you do not already have a `.xo`, produce it with `tapa compile`:

```bash
platform=xilinx_u280_xdma_201920_3

tapa \
  --work-dir work.out \
  compile \
  --top VecAdd \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vadd.$platform.hw.xo
```

The `.xo` file is the artifact that feeds `v++`.

## Stage 2: Link into an FPGA bitstream

```bash
v++ -o vadd.$platform.hw.xclbin \
  --link \
  --target hw \
  --kernel VecAdd \
  --platform $platform \
  vadd.$platform.hw.xo
```

```admonish warning
This step takes **several hours** depending on design complexity and host machine performance. Plan accordingly and consider running it on a dedicated build server (see [Remote Execution](remote-execution.md)).
```

The output artifact is `vadd.$platform.hw.xclbin` — this is the bitstream loaded onto the FPGA.

Key alignment rules:
- `--kernel VecAdd` must match the top-level function name in your TAPA source.
- `--platform $platform` must match the platform string used in `tapa compile --part-num`.
- The input `.xo` filename (`vadd.$platform.hw.xo`) must be the file produced by `tapa compile`.

## Stage 3: Execute on the FPGA

The same host executable used for software and hardware simulation runs on board:

```bash
./vadd --bitstream=vadd.$platform.hw.xclbin
```

## Expected output

```
INFO: Found platform: Xilinx
INFO: Found device: xilinx_u280_xdma_201920_3
INFO: Using xilinx_u250_xdma_201830_2
...
elapsed time: 7.48926 s
PASS!
```

On-board execution is substantially faster than hardware emulation. The elapsed time includes FPGA reconfiguration time (loading the bitstream).

## Validation

The run is correct when:

1. XRT finds and selects the expected device.
2. The elapsed time is reported.
3. The application's correctness check prints `PASS!`.

```admonish tip
If you use `std::vector` for memory-mapped buffers, XRT may warn about unaligned host pointers, which causes an extra memory copy. To eliminate the copy, use `std::vector<T, tapa::aligned_allocator<T>>` instead.
```

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for diagnosis steps. Common issues include XRT not finding the device, platform string mismatches, and bitstream generated for a different platform than the installed board.
```

---

**Next step:** [Remote Execution](remote-execution.md)
