# Fast Hardware Simulation

**Purpose:** Validate RTL correctness faster than Vitis cosimulation using TAPA's fast cosim.

**When to use this:** After `tapa compile` produces an RTL archive and before
the multi-hour `v++ --link` step. For small designs, fast cosim checks the
generated RTL in seconds; Vitis hardware emulation takes much longer to set
up and run.

## What you need

- A `.xo` kernel object from `tapa compile` (or a `.zip` for `xilinx-hls`)
- A host executable built against the same kernel interface
- One of these backends:
  - **xsim**: Requires Vivado on Linux.
  - **verilator**: Open-source, works on Linux and macOS, and does not require
    Vivado. **Must be 5.044 or newer** — see below.

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
./vadd --bitstream vecadd.xo 1000
```

For the `xilinx-hls` target, a `.zip` file also works:

```bash
./vadd --bitstream vecadd.zip 1000
```

### Choosing a simulator backend

The default backend is `xsim`. To switch to Verilator:

```bash
./vadd --bitstream vecadd.xo -cosim_simulator verilator 1000
```

Verilator generates models for supported HLS floating-point and unsigned
divider IP from the packaged Tcl configuration, including the declared
latency and clock enable. Single-precision unfused multiply-add rounds the
product before addition; it does not use fused multiply-add semantics.

For xsim, pass `-xsim_part_num` with the part used for HLS when running an
XO. XO loading does not recover the target part; the fallback is
`xc7a100tcsg324-1`. A different device family can select incorrect models
for device-specific primitives, including V80 floating-point operators.
HLS ZIP archives carry their part number in `tapa.json` and use it unless
overridden.

```bash
./vadd --bitstream VecAdd.xo \
    -xsim_part_num xcv80-lsva4737-2MHP-e-S 1000
```

### Saving waveforms

Specify a persistent work directory and enable waveform saving:

```bash
./vadd --bitstream vecadd.xo \
    -cosim_work_dir ./cosim_work \
    -xsim_save_waveform \
    1000
```

```admonish warning
Strongly recommended: pair `-xsim_save_waveform` with `-cosim_work_dir`. Without a persistent work directory, fast cosim uses a temporary directory that is deleted at exit, removing any saved waveforms with it.
```

### Setup, manual xsim run, and resume

Use this workflow when you need to inspect or modify the generated xsim
environment before running it. The resume step does not simulate by itself:
you must run the simulator between setup and resume.

```bash
cosim="$PWD/cosim_work"

# Step 1: generate the work directory and dpi_config.json, then exit.
./vadd --bitstream=vecadd.xo \
    -cosim_work_dir "$cosim" \
    -cosim_setup_only \
    1000

# Step 2: inspect the files if needed, then run xsim yourself.
TAPA_DPI_CONFIG="$(cat "$cosim/dpi_config.json")" \
  HOME="$cosim/run" \
  vivado -mode batch -source "$cosim/run_cosim.tcl"

# Step 3: reopen the simulated buffers and run host-side checks.
./vadd --bitstream=vecadd.xo \
    -cosim_work_dir "$cosim" \
    -cosim_resume_from_post_sim \
    1000
```

The setup-only command exits before the application's correctness checks.
The final command skips RTL simulation, copies output buffers back, and lets
the application check the results.

### Parallel runs

A pipeline split into separately compiled kernels can run one cosim process
per kernel. Launch those kernels as children of a software-simulated upper
task and pass each archive through `tapa::executable`:

```cpp
DEFINE_string(producer_bitstream, "", "XO for Producer kernel");
DEFINE_string(consumer_bitstream, "", "XO for Consumer kernel");

void Top(/* kernel arguments */) {
  // Declare inter-kernel streams here.
  tapa::task()
      .invoke(Producer, tapa::executable(FLAGS_producer_bitstream), ...)
      .invoke(Consumer, tapa::executable(FLAGS_consumer_bitstream), ...);
}
```

Run the upper task through `tapa::invoke(Top, "", ...)`; the empty path
keeps the orchestration in software while each non-empty executable path
starts RTL cosimulation. Ordinary sequential `tapa::invoke` calls in `main`
do not run concurrently. See [Parallel RTL Emulation](parallel-rtl-emulation.md)
for the complete pattern and leaf-top target requirements.

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
| `-xsim_part_num <part>` | Override the FPGA part recorded in the archive. Usually unnecessary. |
| `-cosim_work_dir <dir>` | Persistent working directory for simulation artifacts. Without this, a temporary directory is used and deleted after the run. |
| `-xsim_save_waveform` | Save xsim waveforms to `wave.wdb`. Pair with `-cosim_work_dir` to keep them after exit. |
| `-xsim_start_gui` | Open the Vivado GUI for interactive debugging during simulation. |
| `-cosim_simulator <backend>` | Simulator backend: `xsim` (default, Linux only) or `verilator` (Linux/macOS). |
| `-cosim_timeout_seconds <seconds>` | Stop fast cosim after this much wall-clock time; `0` (the default) leaves it unlimited. |
| `-cosim_setup_only` | Run simulation setup only, then stop before executing the simulation. |
| `-cosim_resume_from_post_sim` | Skip re-running the simulation; jump directly to post-simulation checks. |
| `-cosim_work_dir_parallel` | Create a unique subdirectory per instance when running concurrent simulations. |

## Expected output

Fast cosim completes in seconds for simple designs. A successful run prints the application's correctness result (e.g., `PASS!`) after the simulation finishes.

## Debugging frozen simulations

Fast cosim has no timeout by default: a large design may legitimately need
hours, so the runtime cannot choose a safe universal limit. For unattended
runs, set an explicit wall-clock limit:

```bash
./vadd --bitstream vecadd.xo -cosim_timeout_seconds 3600 1000
```

On expiry, TAPA terminates the simulator process group and reports which limit
was reached. A timeout does not distinguish a deadlock from a workload that is
merely too large; use a reduced input first, then inspect the saved work
directory if the reduced case also stalls.

If the simulation becomes unresponsive:

1. Use `-cosim_work_dir` and `-xsim_save_waveform` before starting so the
   artifacts survive.
2. Stop the run with Ctrl-C or `-cosim_timeout_seconds`.
3. Open the saved `wave.wdb` in Vivado's waveform viewer. For xsim, its
   default location is
   `<work-dir>/run/cosim.sim/sim_1/behav/xsim/wave.wdb`.

For a new interactive run, pass `-xsim_start_gui`. Do not source
`run_cosim.tcl` merely to inspect an old waveform: the script builds and runs
a new simulation. For xsim post-mortems, the work directory also contains the
`tb_<TOP>.sv` testbench, the `vivado.log` transcript, and the `run/` Vivado
project.

```admonish warning
Cross-channel access for HBM is not currently supported in fast cosimulation. Each AXI interface can only access one HBM channel.
```

## If something goes wrong

```admonish warning
See [Cosimulation Issues](../troubleshoot/cosim-issues.md) for diagnosis steps covering xsim hangs, Verilator build errors, and waveform debugging.
```

---

**Next step:** [Vitis Cosimulation](vitis-cosim.md)
