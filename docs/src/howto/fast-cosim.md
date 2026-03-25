# Fast Hardware Simulation

**Purpose:** Validate RTL correctness faster than Vitis cosimulation using TAPA's fast cosim.

**When to use this:** After `tapa compile` produces a `.xo` file, before the multi-hour `v++ --link` step. Fast cosim catches logic bugs in generated RTL in seconds rather than the ten-plus minutes Vitis cosimulation requires.

## What you need

- A `.xo` kernel object from `tapa compile` (or a `.zip` for the `xilinx-hls` target)
- One of:
  - **xsim**: Requires a Vivado installation. Linux only.
  - **verilator**: Open-source. Works on Linux and macOS. No Vivado required.

## Commands

### Basic run

Pass the `.xo` file as the `--bitstream` argument:

```bash
./vadd --bitstream VecAdd.xo 1000
```

For the `xilinx-hls` target, a `.zip` file also works:

```bash
./vadd --bitstream VecAdd.zip 1000
```

### Choosing a simulator backend

The default backend is `xsim`. To switch to Verilator:

```bash
./vadd --bitstream VecAdd.xo -xosim_simulator verilator 1000
```

To invoke `tapa cosim` directly:

```bash
tapa cosim --simulator verilator ...
```

### Saving waveforms

Specify a persistent work directory and enable waveform saving:

```bash
./vadd --bitstream VecAdd.xo \
    -xosim_work_dir ./cosim_work \
    -xosim_save_waveform \
    1000
```

```admonish warning
Strongly recommended: pair `-xosim_save_waveform` with `-xosim_work_dir`. Without a persistent work directory, fast cosim uses a temporary directory that is deleted at exit, removing any saved waveforms with it.
```

### Setup-only and resume workflow

When you want to inspect the generated simulation environment before committing to a full run:

```bash
# Step 1: set up the simulation environment and stop before running
./vadd --bitstream VecAdd.xo \
    -xosim_work_dir ./cosim_work \
    -xosim_setup_only \
    1000

# Step 2: after inspecting, run post-simulation checks without re-simulating
./vadd --bitstream VecAdd.xo \
    -xosim_work_dir ./cosim_work \
    -xosim_resume_from_post_sim \
    1000
```

### Parallel runs

When running multiple fast-cosim instances concurrently, prevent work-directory collisions by using a dedicated flag:

```bash
./vadd --bitstream VecAdd.xo \
    -xosim_work_dir ./cosim_work \
    -xosim_work_dir_parallel_cosim \
    1000
```

Each instance creates a uniquely named subdirectory within `./cosim_work`.

## Runtime flags reference

The following flags control fast cosim behavior when passed to the host executable. The canonical reference is [Runtime Flags](../reference/runtime-flags.md).

| Flag | Description |
|------|-------------|
| `-xosim_executable <path>` | Path to the `tapa-fast-cosim` binary when it is not in `PATH`. |
| `-xosim_part_num <part>` | Target FPGA part number for simulation (e.g., `xcu280-fsvh2892-2L-e`). |
| `-xosim_work_dir <dir>` | Persistent working directory for simulation artifacts. Without this, a temporary directory is used and deleted after the run. |
| `-xosim_save_waveform` | Save simulation waveforms to a `.wdb` file in the work directory. Requires `-xosim_work_dir`. |
| `-xosim_start_gui` | Open the Vivado GUI for interactive debugging during simulation. |
| `-xosim_simulator <backend>` | Simulator backend: `xsim` (default, Linux only) or `verilator` (cross-platform). |
| `-xosim_setup_only` | Run simulation setup only, then stop before executing the simulation. |
| `-xosim_resume_from_post_sim` | Skip re-running the simulation; jump directly to post-simulation checks. |
| `-xosim_work_dir_parallel_cosim` | Create a unique subdirectory per instance when running concurrent simulations. |

## Expected output

Fast cosim completes in seconds for simple designs. A successful run prints the application's correctness result (e.g., `PASS!`) after the simulation finishes.

## Debugging frozen simulations

If the simulation becomes unresponsive:

1. Run with `-xosim_work_dir` to persist intermediate files.
2. Abort the simulation with Ctrl-C.
3. Locate `[work-dir]/output/run/run_cosim.tcl`.
4. Open Vivado in GUI mode and source the script:
   ```bash
   vivado -mode gui -source [work-dir]/output/run/run_cosim.tcl
   ```

This allows real-time observation and waveform analysis of the frozen state.

```admonish warning
Cross-channel access for HBM is not currently supported in fast cosimulation. Each AXI interface can only access one HBM channel.
```

## If something goes wrong

```admonish warning
See [Cosimulation Issues](../troubleshoot/cosim-issues.md) for diagnosis steps covering xsim hangs, Verilator build errors, and waveform debugging.
```

---

**Next step:** [Vitis Cosimulation](vitis-cosim.md)
