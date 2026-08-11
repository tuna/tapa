# Vitis Cosimulation

**Purpose:** Run full Vitis hardware emulation for accurate timing after fast cosim passes.

**When to use this:** When you need accurate timing or bandwidth numbers that fast cosim cannot provide. This step is slow (5–10 minutes for simple designs) and is rarely the first choice — run [Fast Hardware Simulation](fast-cosim.md) first to catch logic errors.

## What you need

- A `.xo` kernel object from `tapa compile`
- Vitis and XRT installed (Linux only)
- The target platform string (e.g., `xilinx_u55c_gen3x16_xdma_3_202210_1`)

## Commands

### Generate the hardware emulation bitstream

```bash
platform=xilinx_u55c_gen3x16_xdma_3_202210_1

v++ -o vadd.$platform.hw_emu.xclbin \
  --link \
  --target hw_emu \
  --kernel VecAdd \
  --platform $platform \
  vadd.$platform.hw.xo
```

Replace `$platform` with your actual target platform string and `VecAdd` with your top-level kernel name. This step typically takes 5–10 minutes.

### Run the hardware emulation

```bash
./vadd --bitstream=vadd.$platform.hw_emu.xclbin 1000
```

The same host executable used for software simulation and fast cosim runs unchanged here — only the `--bitstream` argument changes.

## Expected output

```
INFO: [HW-EMU 05] Path of the simulation directory : /tmp/.frt.0/.run/.../behav_waveform/xsim
INFO: [HW-EMU 01] Hardware emulation runs simulation underneath. Using a large data set will result in long simulation times. It is recommended that a small dataset is used for faster execution. The flow uses approximate models for Global memories and interconnect and hence the performance data generated is approximate.
XRT build version: 2.20.0
...
kernel time: 1.00021 s
PASS!
INFO: [HW-EMU 06-0] Waiting for the simulator process to exit
INFO: [HW-EMU 06-1] All the simulator processes exited successfully
```

```admonish warning title="The reported time is not a measurement"
Under hardware emulation the printed `kernel time` is sampled on a one-second
poll, so it rounds up to a whole second: the same `vadd` xclbin reports
`1.00024 s` for `n=100` and `4.00121 s` for `n=10000`. Read it as confirmation
that the kernel ran, not as a number to compare designs with. For cycle-level
timing, use the simulation log under the path in the `HW-EMU 05` line.
```

```admonish note
Vitis hardware emulation uses approximate models for DDR memory and interconnects. Performance numbers from `hw_emu` are indicative, not exact. For precise measurements, run on an actual board using an `hw` bitstream.
```

## Validation

The run is correct when:

1. The `INFO: [HW-EMU 06-1] All the simulator processes exited successfully` line appears.
2. The application's correctness check prints `PASS!`.
3. A kernel time is reported at all — which confirms the kernel executed, but see the warning above before reading anything into its value.

```admonish tip
Use a small dataset for hardware emulation runs. Large datasets cause proportionally long simulation times because every clock cycle is simulated in software.
```

## If something goes wrong

```admonish warning
See [Cosimulation Issues](../troubleshoot/cosim-issues.md) for diagnosis steps. Common issues include missing XRT environment variables, platform string mismatches, and kernel name mismatches between the `--kernel` flag and the TAPA top-level function name.
```

---

**Next step:** [Build & Run on Board](build-and-run.md)
