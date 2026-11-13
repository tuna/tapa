# Cosimulation Issues

**When to use this page:** When `--bitstream=vadd.xo` (fast cosim) runs differently from software simulation, or when cosim produces xsim or Verilator errors.

---

## Fast cosim vs software simulation mismatches

If fast cosim fails (`FAIL!` or hangs) but software simulation passes, the most common causes are:

- **Scheduling differences can expose missing synchronization.** Software
  simulation interleaves coroutines over one or more workers; RTL advances
  every task in parallel each cycle. An ordering assumption that happens to
  hold in one software run may not hold in RTL. Fix: enforce ordering with
  stream or memory handshakes rather than relying on scheduling.

- **Blocking `async_mmap` operations inside pipelined loops.** A blocking call inside a pipelined loop can stall the pipeline in RTL in ways that software simulation does not model. Fix: use non-blocking reads/writes and manually handle the response FIFOs, or switch to `tapa::mmap` to simplify the memory access model while debugging.

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

## Fast cosim runs indefinitely

A run that never reaches `ap_done` may be deadlocked, but cycle-accurate
simulation of a valid full-size workload can also take hours. TAPA therefore
does not impose a timeout by default. For unattended runs, choose a limit that
fits the workload:

```bash
./app --bitstream=kernel.xo -cosim_timeout_seconds=3600
```

When the limit expires, the runtime terminates the xsim or Verilator process
group and reports an explicit wall-clock-timeout error. Retry with a small
input before treating the timeout as a design bug. If the small case also
stalls, keep a `-cosim_work_dir`, inspect the stream-progress diagnostics
below, and debug the saved waveform. The timeout itself cannot identify which
task or protocol is blocked.

---

## `kernel.xml places arg ... at register offset ...`

On a card, XRT writes each argument at the offset the `.xo`'s `kernel.xml`
gives it, while the kernel decodes it where its control slave
(`<top>_control_s_axi.v`) puts it. Fast cosim checks that the two agree for
every argument and refuses a package whose offsets differ, because that
kernel would read the wrong registers on hardware. An `.xo` packed by an
older TAPA whose top takes an arbitrary-precision scalar wider than 64 bits
(`tapa::u<80>`, `ap_uint<80>`) can have this mismatch, as can one whose
`kernel.xml` was edited by hand. Rebuild the `.xo` with the current `tapa`.

---

## xsim issues

### ``cosim error: simulator tool `vivado` not found``

xsim is part of the Vivado installation, and fast cosim drives it through the
`vivado` on `PATH`. Source the Vivado environment script before running cosim:

```bash
source /opt/Xilinx/Vivado/2024.2/settings64.sh
./vadd --bitstream=vadd.xo ...
```

Adjust the path to match your Vivado installation and version.

### `xsim hangs at elaboration`

Check that the `.xo` file was produced by a successful `tapa compile` run. A partial or corrupt `.xo` (from a failed or interrupted compilation) can cause elaboration to hang silently. Re-run `tapa compile` from scratch and verify it exits with status 0 before running cosim.

### Segfault inside xsim

Check the xsim and DPI logs for malformed RTL, a failing custom module, or a
tool crash. Verify that the XO completed packaging successfully and that any
custom RTL passes independent lint/simulation. If the evidence points to the
simulator itself, retry with a supported Vitis/Vivado version; the repository
CI configuration lists the versions TAPA tests.

---

## Verilator issues

### ``cosim error: simulator tool `verilator` not found``

Verilator is not on `PATH` and `VERILATOR_BIN` is unset. Install it, or set
`VERILATOR_BIN` to a Verilator executable — but read the next section first
and check the version you get, because the package managers ship releases too
old for TAPA and the resulting failure looks like something else entirely:

```bash
# Debian/Ubuntu — check `verilator --version` afterwards
sudo apt install verilator
```

### Verilator compilation error (Verilog parsing error)

TAPA needs **Verilator 5.044 or newer** — the version pinned in the repository's
`MODULE.bazel` and used by CI. Older releases reject the AXI master that Vitis
HLS generates for any `mmap` port, so even the `vadd` example fails:

```
%Error: rtl/Stream2Mmap_mmap_m_axi.v:2046:27: Expecting expression to be
        constant, but can't determine constant for FUNCREF 'log2'
```

Distribution packages lag well behind (Ubuntu 24.04 ships 5.020), so check
`verilator --version` and [build from source](https://verilator.org/guide/latest/install.html)
if it is older. The xsim backend has no such constraint.

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

Then open
`./cosim_work/run/cosim.sim/sim_1/behav/xsim/wave.wdb` in Vivado's
waveform viewer. Do not source `run_cosim.tcl` merely to inspect an old
waveform; that starts a new simulation.

Add AXI memory signals and compare expected and actual data on each
transaction. Look for read data that does not match host input or writes to
unexpected addresses.

---

## Stream diagnostics

After repeated stalled operations, the DPI runtime can report stream
progress roughly every 10 seconds. These messages appear on stderr and
include the port name and queue state:

```
frt-dpi: progress[a_fifo_s]: read_ok=16 read_empty=40000000 write_ok=0 write_full=0 q_head=8 q_tail=8
```

| Field | Meaning |
|-------|---------|
| `progress[port]` | The port that triggered the report (the one currently stalling). |
| `read_ok` | Total successful reads across all ports in this process. |
| `read_empty` | Total empty-read attempts (queue had no data). |
| `write_ok` | Total successful writes across all ports. |
| `write_full` | Total full-write attempts (queue had no space). |
| `q_head` / `q_tail` | Shared-memory queue counters for the stalling port. `q_tail` = elements pushed by the producer; `q_head` = elements popped by the consumer. `q_head == q_tail` means the queue is empty. |

### Enabling verbose per-element logging

Set the `FRT_STREAM_DEBUG` environment variable to log every successful stream read and write:

```bash
FRT_STREAM_DEBUG=1 ./vadd --bitstream=vadd.xo 1000
```

### Interpreting stall patterns

- **`q_tail=0`** on a consumer port: the producer never wrote to this stream. Check that the producer's xsim started and that stream arguments are bound correctly.
- **`q_head == q_tail` but `read_ok < expected`**: all produced elements were consumed but not enough were produced. The producer may have exited before flushing all writes.
- **`write_full` growing**: the consumer is not draining the queue fast
  enough, or the queue is too small for the burst. Check the consumer and
  stream protocol. `TAPA_CONCURRENCY` controls host workers, not simulator
  speed, so increasing it is not a general fix.

---

```admonish tip
Always pass software simulation before running fast cosim. Software simulation runs faster and catches logic bugs in C++. Fast cosim catches RTL bugs introduced by synthesis and scheduling. Skipping software simulation wastes cosim time on bugs that are much faster to fix at the C++ level.
```

---

**See also:** [Common Errors](common-errors.md) | [Deadlocks & Hangs](deadlocks-and-hangs.md)
