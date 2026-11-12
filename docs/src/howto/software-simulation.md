# Software Simulation

**Purpose:** Run software simulation to verify your TAPA design's logic without FPGA hardware.

**When to use this:** Before synthesizing — software simulation is fast (seconds) and requires only a C++ compiler and the TAPA library.

## What you need

- A compiled TAPA host executable (produced by `tapa g++`)
- No FPGA, no Vivado, no XRT required
- No vendor headers: the host runtime is self-contained

```admonish note
Software simulation does not need Vitis HLS headers. Use TAPA's portable
types (`tapa::u`, `tapa::i`, `tapa::fixed`, `tapa::axis`), `tapa::wait()`, and
`[[tapa::...]]` attributes instead of vendor-specific headers and intrinsics.
`wait()` and synthesis attributes have no effect on the host. A direct vendor
header include ties the host build to a sourced Vitis: `tapa g++` adds the
Vitis HLS include directory only when `XILINX_HLS` or `XILINX_VITIS` is set.

When `tapa analyze` recognizes a vendor construct, it reports a remark with
the portable spelling; the usage remark itself is not an error. Other
syntactic or semantic errors can still reject the program. See the
[vendor-usage warnings](../reference/cli.md#vendor-usage-warnings).
```

## Commands

Run the executable with no `--bitstream` argument. TAPA detects the missing argument and runs the software simulation:

```bash
./vadd
```

To make ordering-sensitive failures easier to reproduce, use one worker
thread:

```bash
TAPA_CONCURRENCY=1 ./vadd
```

```admonish note
`TAPA_CONCURRENCY` defaults to the physical CPU cores the process may run
on: under `taskset` or a batch scheduler's allocation, only those. A value
below 1 means 1. Set it to `1` for a simpler scheduling interleaving at the
cost of simulation speed. One
worker still interleaves tasks at blocking points; it does not run each task
to completion. On macOS the host library runs each task on a thread of its
own and ignores `TAPA_CONCURRENCY`.
```

## Expected output

```
WARNING: Logging before InitGoogleLogging() is written to STDERR
I... task.h:...] running software simulation with TAPA library
kernel time: 0.0032 s
PASS!
```

The `running software simulation` line confirms the software simulation path was taken. `PASS!` is printed by the application when its correctness check succeeds. The leading `WARNING:` line comes from glog and is harmless. Exact timestamps, source line numbers, and timings vary per run and per build.

## Stream logging

To capture values written to named `tapa::stream` channels, set
`TAPA_STREAM_LOG_DIR` to an existing directory before running:

```bash
mkdir -p /tmp/tapa-stream-logs
TAPA_STREAM_LOG_DIR=/tmp/tapa-stream-logs ./vadd
```

TAPA writes one log file per named stream, named after where the stream sits
in the design, so two instances of one task never share a file:

- A stream the top-level task declares logs as `<name>.txt`.
- A stream another task instance declares logs as `<path>.<name>.txt`. The
  path joins, with `.`, one component per instance from the top down: the
  invoke's name when it has one (`.invoke(Mid, "mid", ...)`; instance `i` of
  a repeated named invoke is `mid_i`), else `<Task>_<k>`, the name of the
  task's function and `k` counting the parent's invokes of that task from 0.
  An `Outer` invoked twice, each invoking `Mid` twice, logs its stream `q` as
  `Outer_0.Mid_0.q.txt`, `Outer_0.Mid_1.q.txt`, `Outer_1.Mid_0.q.txt` and
  `Outer_1.Mid_1.q.txt`.
- An `async_mmap`'s channels log as
  `<parent path>.<invoke>.<parameter>.<channel>.txt`: the path of the task
  invoking it, the invoke's name or else its index among that task's
  invokes counted from 0, the position of the `async_mmap` parameter, and
  `read_addr`, `read_data`, `write_addr`, `write_data` or `write_resp`. So a
  leaf's component is its invoke index unless the invoke is named; naming it
  gives readable paths: `.invoke(Copy, "copy", src, dst)` from the top logs
  `copy.0.read_addr.txt` where the unnamed fourth invoke logs
  `3.0.read_addr.txt`.

The names are the same on every run and with either scheduler. A task's
name is that of the function whose body declares the stream or makes the
invokes. Unnamed streams are not logged. The format depends on the element
type:

- **Primitive types** (`int`, `float`, …) are logged as human-readable text, one value per line. For example, writing `42` to a `tapa::stream<int>` produces `42\n`.
- **Non-primitive types without `operator<<`** are logged as raw object bytes
  in host byte order (little-endian on supported hosts). For example, writing
  `Foo{0x4222}` to a `tapa::stream<Foo>` produces `0x22420000\n` on a
  little-endian host.
- **Non-primitive types with `operator<<` defined** are logged using that operator, producing human-readable text.

## What software simulation does—and does not—model

Vitis HLS C simulation executes dataflow functions sequentially rather than
reproducing their cycle-by-cycle concurrency. TAPA instead schedules each task
as a coroutine. A blocking stream operation yields so another task can run;
worker threads (`TAPA_CONCURRENCY`, defaulting to the physical cores the
process may run on) can
also run tasks in parallel. With `tapa::stream<T, Depth>`, the same depth
limits apply, so producers experience backpressure when a FIFO fills.

An invocation waits for all joined tasks, including descendants scheduled by
upper tasks on other workers. Detached tasks do not hold up completion, nor do
the tasks a detached upper task invokes, as in hardware. They end with the
invocation instead: each unwinds at the next stream operation that finds its
stream empty or full.

This helps expose missing handshakes, element-count mismatches, and ordering
assumptions before synthesis. It is not cycle-accurate: it does not model HLS
scheduling, clock timing, or every per-instance RTL state detail. A passing
software simulation therefore checks the C++ dataflow contract, not every
property of the generated hardware. Use fast cosimulation for RTL validation.

## Debugging with GDB

Software simulation runs as ordinary host code, so GDB (and sanitizers) work
without any special setup — see [Your First Debug
Cycle](../start/first-debug.md) for the workflow.

## Validation

Simulation is correct when:

1. The program exits with code 0.
2. The application's own correctness check prints `PASS!` (or your application's equivalent).
3. No deadlock or hang occurs within the expected runtime.

## If something goes wrong

```admonish warning
If the simulation hangs or runs much longer than expected, a stream deadlock
is one possibility. See [Deadlocks & Hangs](../troubleshoot/deadlocks-and-hangs.md)
for diagnosis steps.

For unexpected errors or assertion failures, see [Common Errors](../troubleshoot/common-errors.md).
```

---

**Next step:** [Fast Hardware Simulation](fast-cosim.md)
