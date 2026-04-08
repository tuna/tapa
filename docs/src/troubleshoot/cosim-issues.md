# Cosimulation Issues

**When to use this page:** When `--bitstream=vadd.xo` (fast cosim) runs differently from software simulation, or when cosim produces xsim or Verilator errors.

---

## Fast cosim vs software simulation mismatches

If fast cosim fails (`FAIL!` or hangs) but software simulation passes, the most common causes are:

- **Non-deterministic scheduling can expose races not visible in software simulation.** Software simulation uses coroutine scheduling that runs tasks cooperatively; RTL runs tasks truly in parallel. Races that are hidden by cooperative scheduling in software simulation may surface as failures in fast cosim. Fix: remove any assumptions about task ordering that are not enforced by stream synchronization.

- **Blocking `async_mmap` operations inside pipelined loops.** A blocking call inside a pipelined loop can stall the pipeline in RTL in ways that software simulation does not model. Fix: use non-blocking reads/writes and manually handle the response FIFOs, or switch to `tapa::mmap` to simplify the memory access model while debugging.

- **Non-deterministic task scheduling.** Software simulation uses coroutine scheduling that may resolve races differently than RTL. If results depend on the relative timing of two tasks, they may differ between simulation and RTL.

```admonish note
Fast cosim models DRAM with a simplified functional model. Throughput and latency numbers from fast cosim are not representative of on-board performance. Use fast cosim only to verify functional correctness.
```

---

## HBM cross-channel access limitation

```admonish warning
Fast cosimulation does not support cross-channel access for HBM. Each AXI interface can only access one HBM channel. Designs that require cross-channel HBM access must be validated on hardware rather than in fast cosim.
```

If your design uses multiple HBM pseudo-channels and the fast cosim result does not match software simulation, verify that no single AXI port accesses more than one HBM channel.

---

## xsim issues

### `xsim not found` or `Vivado not found`

xsim is part of the Vivado installation. Source the Vivado environment script before running cosim:

```bash
source /opt/Xilinx/Vivado/2022.1/settings64.sh
./vadd --bitstream=vadd.xo ...
```

Adjust the path to match your Vivado installation and version.

### `xsim hangs at elaboration`

Check that the `.xo` file was produced by a successful `tapa compile` run. A partial or corrupt `.xo` (from a failed or interrupted compilation) can cause elaboration to hang silently. Re-run `tapa compile` from scratch and verify it exits with status 0 before running cosim.

### Segfault inside xsim

This is typically a Vivado bug. Try switching to a different Vitis/Vivado version. Versions tested by the TAPA CI pipeline are listed in the TAPA repository's CI configuration.

---

## Verilator issues

### `verilator not found`

Install Verilator from your package manager or build from source:

```bash
# Debian/Ubuntu
sudo apt install verilator
```

### Verilator compilation error (Verilog parsing error)

TAPA generates Verilog targeting recent Verilator versions. If you see Verilog parsing errors, update Verilator to the version used in TAPA's CI pipeline.

### No waveform support with Verilator

Verilator simulation does not support waveform capture via the Vivado GUI. If you need waveform debugging, use xsim and pass `-xsim_save_waveform` as described below.

---

## Cosim produces wrong output (`FAIL!`) but xsim does not hang

Run with waveform capture and a persistent work directory so you can inspect the simulation after it completes:

```bash
./vadd --bitstream=vadd.xo \
  -cosim_work_dir ./cosim_work \
  -xsim_save_waveform \
  1000
```

Then open the waveform in Vivado GUI:

```bash
vivado -mode gui -source ./cosim_work/output/run/run_cosim.tcl
```

In the waveform viewer, add the AXI memory interface signals and compare the expected vs actual data on each transaction. Look for read data that does not match what the host wrote, or write transactions that target unexpected addresses.

---

```admonish tip
Always pass software simulation before running fast cosim. Software simulation runs faster and catches logic bugs in C++. Fast cosim catches RTL bugs introduced by synthesis and scheduling. Skipping software simulation wastes cosim time on bugs that are much faster to fix at the C++ level.
```

---

**See also:** [Common Errors](common-errors.md) | [Deadlocks & Hangs](deadlocks-and-hangs.md)
