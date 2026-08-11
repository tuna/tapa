# Fast Hardware Simulation

**Purpose:** Validate RTL correctness faster than Vitis cosimulation using TAPA's fast cosim.

**When to use this:** After `tapa compile` produces a `.xo` file, before the multi-hour `v++ --link` step. Fast cosim catches logic bugs in generated RTL in seconds rather than the ten-plus minutes Vitis cosimulation requires.

## What you need

- A `.xo` kernel object from `tapa compile` (or a `.zip` for the `xilinx-hls` target)
- One of:
  - **xsim**: Requires a Vivado installation. Linux only. No version floor.
  - **verilator**: Open-source. Works on Linux and macOS. No Vivado required.
    **Must be 5.044 or newer** — see below.

```admonish warning title="Check your Verilator version before choosing it"
Verilator older than 5.044 cannot elaborate the AXI master Vitis HLS generates
for an `mmap` port, so every design with one fails — including `vadd`. The
distribution packages are well behind that floor (Ubuntu 24.04 ships 5.020), so
`apt install verilator` is usually not enough; check `verilator --version` and
[build from source](https://verilator.org/guide/latest/install.html) if it is
older. The failure looks like a Verilog parsing error rather than a version
problem:

    %Error: rtl/Stream2Mmap_mmap_m_axi.v:2046:27: Expecting expression to be
            constant, but can't determine constant for FUNCREF 'log2'
```

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
./vadd --bitstream VecAdd.xo -cosim_simulator verilator 1000
```

### Saving waveforms

Specify a persistent work directory and enable waveform saving:

```bash
./vadd --bitstream VecAdd.xo \
    -cosim_work_dir ./cosim_work \
    -xsim_save_waveform \
    1000
```

```admonish warning
Strongly recommended: pair `-xsim_save_waveform` with `-cosim_work_dir`. Without a persistent work directory, fast cosim uses a temporary directory that is deleted at exit, removing any saved waveforms with it.
```

### Setup-only and resume workflow

When you want to inspect the generated simulation environment before committing to a full run:

```bash
# Step 1: set up the simulation environment and stop before running
./vadd --bitstream VecAdd.xo \
    -cosim_work_dir ./cosim_work \
    -cosim_setup_only \
    1000

# Step 2: after inspecting, run post-simulation checks without re-simulating
./vadd --bitstream VecAdd.xo \
    -cosim_work_dir ./cosim_work \
    -cosim_resume_from_post_sim \
    1000
```

### Parallel runs

When a host application calls `tapa::invoke` more than once — for example, a pipeline split into separate kernels each compiled to its own `.xo` file — TAPA launches all cosim instances concurrently. Each kernel is compiled independently and its `.xo` path is passed to its own `tapa::invoke` call via a separate bitstream flag:

```cpp
// Host code: two separate kernels, each with its own bitstream flag
DEFINE_string(producer_bitstream, "", "XO for Producer kernel");
DEFINE_string(consumer_bitstream, "", "XO for Consumer kernel");

tapa::invoke(Producer, FLAGS_producer_bitstream, ...);
tapa::invoke(Consumer, FLAGS_consumer_bitstream, ...);
```

```bash
./app --producer_bitstream=producer.xo --consumer_bitstream=consumer.xo
```

If all instances share the same `-cosim_work_dir`, their simulation environments collide. Pass `-cosim_work_dir_parallel` to give each instance its own uniquely named subdirectory:

```bash
./app \
    --producer_bitstream=producer.xo \
    --consumer_bitstream=consumer.xo \
    -cosim_work_dir ./cosim_work \
    -cosim_work_dir_parallel
```

TAPA creates `./cosim_work/XXXXXX/` (a unique name per instance) so that the simulations run without interfering with each other's build artifacts.

## Runtime flags reference

The following flags control fast cosim behavior when passed to the host executable. The canonical reference is [Runtime Flags](../reference/runtime-flags.md).

| Flag | Description |
|------|-------------|
| `-xsim_part_num <part>` | Target FPGA part number for simulation (e.g., `xcu55c-fsvh2892-2L-e`). |
| `-cosim_work_dir <dir>` | Persistent working directory for simulation artifacts. Without this, a temporary directory is used and deleted after the run. |
| `-xsim_save_waveform` | Save simulation waveforms to a `.wdb` file in the work directory. Requires `-cosim_work_dir`. |
| `-xsim_start_gui` | Open the Vivado GUI for interactive debugging during simulation. |
| `-cosim_simulator <backend>` | Simulator backend: `xsim` (default, Linux only) or `verilator` (cross-platform). |
| `-cosim_setup_only` | Run simulation setup only, then stop before executing the simulation. |
| `-cosim_resume_from_post_sim` | Skip re-running the simulation; jump directly to post-simulation checks. |
| `-cosim_work_dir_parallel` | Create a unique subdirectory per instance when running concurrent simulations. |

## Expected output

Fast cosim completes in seconds for simple designs. A successful run prints the application's correctness result (e.g., `PASS!`) after the simulation finishes.

## Debugging frozen simulations

If the simulation becomes unresponsive:

1. Run with `-cosim_work_dir` to persist intermediate files.
2. Abort the simulation with Ctrl-C.
3. Open Vivado in GUI mode and source the run script the work directory kept:
   ```bash
   vivado -mode gui -source [work-dir]/run_cosim.tcl
   ```

This allows real-time observation and waveform analysis of the frozen state.

The work directory holds the rest of what a post-mortem needs alongside it:
`tb_<TOP>.sv` is the generated testbench, `vivado.log` is the transcript of the
run that hung, and `run/` is the Vivado project itself
(`run/cosim.sim/sim_1/behav/xsim/` is where `-xsim_save_waveform` writes
`wave.wdb`).

```admonish warning
Cross-channel access for HBM is not currently supported in fast cosimulation. Each AXI interface can only access one HBM channel.
```

## If something goes wrong

```admonish warning
See [Cosimulation Issues](../troubleshoot/cosim-issues.md) for diagnosis steps covering xsim hangs, Verilator build errors, and waveform debugging.
```

---

**Next step:** [Vitis Cosimulation](vitis-cosim.md)
