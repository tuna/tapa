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
    Vivado. **Must be 5.044 or newer**.

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

Verilator must be 5.044 or newer: older releases reject the AXI master that
Vitis HLS generates for an `mmap` port, and the failure surfaces as a
misleading Verilog parsing error. See [Cosimulation
Issues](../troubleshoot/cosim-issues.md) for the full diagnosis.

Verilator generates models for supported HLS floating-point and unsigned
divider IP from the packaged Tcl configuration, including the declared
latency and clock enable. Single-precision unfused multiply-add rounds the
product before addition; it does not use fused multiply-add semantics.

Cosim reads the target part from the archive's `tapa.json` for both XO and
HLS ZIP inputs, so `-xsim_part_num` is usually unnecessary. Pass it only to
override the recorded part; a different device family can select incorrect
models for device-specific primitives, including V80 floating-point
operators.

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

The flags controlling fast cosim behavior are documented in the canonical
[Runtime Flags](../reference/runtime-flags.md#host-executable-flags-fast-cosim)
reference.

## Expected output

Fast cosim completes in seconds for simple designs. A successful run prints the application's correctness result (e.g., `PASS!`) after the simulation finishes.

## If something goes wrong

```admonish warning
See [Cosimulation Issues](../troubleshoot/cosim-issues.md) for diagnosis steps covering frozen or never-ending runs, xsim hangs, Verilator build errors, HBM limitations, and waveform debugging.
```

---

**Next step:** [Vitis Cosimulation](vitis-cosim.md)
