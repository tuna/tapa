# Build & Run on Board

**Purpose:** Build a TAPA design into an FPGA bitstream and run it on an Alveo board.

**When to use this:** After fast cosim (and optionally Vitis cosim) passes — this step converts your `.xo` kernel object into a hardware bitstream and executes it on real silicon.

## What you need

- A `.xo` kernel object from `tapa compile` and its matching host executable
- Vitis and XRT installed (Linux only)
- The target platform string (e.g., `xilinx_u55c_gen3x16_xdma_3_202210_1`)
- An Alveo board installed in the system for the final execution step
- Several hours of compute time for full implementation

For the V80 SLASH service shell, `tapa link` produces a `.vbin` for VRT;
the XRT flow below produces an `.xclbin` for U-series devices.

The VRT runtime is selected by the `.vbin` suffix. Build the optional VRT
adapter against the installed SLASH headers, then select the board and kernel
instance through the existing TAPA host flags:

```bash
cmake -S fpga-runtime/frt/vrt -B build/frt-vrt
cmake --build build/frt-vrt
./callipepla --bitstream=callipepla.vbin \
  --vrt_bdf=0000:a1:00 --vrt_kernel=Callipepla_0 \
  --vrt_library=$PWD/build/frt-vrt/libfrt_vrt.so matrix.mtx 16
```

`tapa link --xo ... --connectivity ... --output ... --clock-hz 100000000`
invokes SLASH and rejects a VBIN whose final routed timing report is below the
requested clock.

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

The `.xo` file feeds `v++ --link` on U-series devices or `tapa link` on V80.

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
- `--kernel VecAdd` must match the top-level function name in your TAPA source, the same name you passed to `tapa compile --top`.
- `--platform $platform` must name a platform built for the same device that
  `tapa compile` targeted; a mismatch fails at link time. The example passes
  the same platform to both commands, so TAPA and `v++` derive consistent
  device settings.
- The input `.xo` filename (`vadd.$platform.hw.xo`) must be the file produced by `tapa compile`.

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
