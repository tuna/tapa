# Runtime Flags

This page covers environment variables and host executable flags that control TAPA behavior at runtime. These apply after compilation, during software simulation or fast hardware cosimulation.

---

## Environment Variables

These variables are read by the host executable at startup.

| Variable | Default | Description |
|----------|---------|-------------|
| `TAPA_CONCURRENCY` | Number of CPU cores | Number of parallel coroutine threads used by software simulation. Set to `1` for single-threaded, more reproducible simulation runs. Has no effect on HLS compilation parallelism (`-j`). |
| `TAPA_STREAM_LOG_DIR` | (unset — logging disabled) | Directory for stream transfer logs. When set, TAPA writes one log file per named stream recording each value written to that stream. Useful for tracing data corruption during software simulation. |

### Example: reproducible single-threaded simulation

```bash
TAPA_CONCURRENCY=1 ./vadd
```

### Example: enable stream logging

```bash
TAPA_STREAM_LOG_DIR=/tmp/stream-logs ./vadd
```

See [Software Simulation](../howto/software-simulation.md) for more on stream logging and debugging.

---

## Host Executable Flags (Fast Cosim)

When the host executable is invoked with `--bitstream=vadd.xo`, it runs fast hardware cosimulation instead of software simulation. The following flags control cosim behavior. They are passed directly on the host executable command line.

```admonish note
These flags use single-dash prefix (e.g., `-xosim_work_dir`) because they are forwarded to the underlying `tapa-fast-cosim` tool via gflags.
```

| Flag | Description |
|------|-------------|
| `-xosim_executable <path>` | Path to the `tapa-fast-cosim` binary when it is not in `PATH`. |
| `-xosim_part_num <part>` | Target FPGA part number for simulation (e.g., `xcu280-fsvh2892-2L-e`). |
| `-xosim_work_dir <dir>` | Persistent working directory for simulation artifacts. Without this flag, a temporary directory is used and deleted after the run. |
| `-xosim_save_waveform` | Save simulation waveforms to a `.wdb` file in the work directory. Pair with `-xosim_work_dir`; without it, the temporary directory and all waveforms are deleted after the run. |
| `-xosim_start_gui` | Open the Vivado GUI for interactive debugging during simulation. |
| `-xosim_simulator <backend>` | Simulator backend: `xsim` (default, Linux only, requires Vivado) or `verilator` (cross-platform, no Vivado required). |
| `-xosim_setup_only` | Run simulation setup only, then stop before executing the simulation. Useful for inspecting generated simulation files before committing to a full run. |
| `-xosim_resume_from_post_sim` | Skip re-running the simulation and jump directly to post-simulation checks. Use after a completed simulation to re-run checks without re-simulating. |
| `-xosim_work_dir_parallel_cosim` | Create a unique subdirectory per instance when running multiple concurrent simulations, preventing work directory collisions. |

### Example: save waveforms from a named work directory

```bash
./vadd --bitstream vadd.xo \
  -xosim_work_dir ./cosim_work \
  -xosim_save_waveform \
  1000
```

### Example: staged workflow (setup then resume)

```bash
# Step 1: set up and inspect the simulation environment
./vadd --bitstream vadd.xo -xosim_work_dir ./cosim_work -xosim_setup_only 1000

# Step 2: run post-simulation checks without re-simulating
./vadd --bitstream vadd.xo -xosim_work_dir ./cosim_work -xosim_resume_from_post_sim 1000
```

For a full walkthrough of fast cosim workflows, see [Fast Hardware Simulation](../howto/fast-cosim.md).
