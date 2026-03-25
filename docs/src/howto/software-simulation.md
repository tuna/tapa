# Software Simulation

**Purpose:** Run software simulation to verify your TAPA design's logic without FPGA hardware.

**When to use this:** Before synthesizing — software simulation is fast (seconds) and requires only a C++ compiler and the TAPA library.

## What you need

- A compiled TAPA host executable (produced by `tapa g++`)
- No FPGA, no Vivado, no XRT required

## Commands

Run the executable with no `--bitstream` argument. TAPA detects the missing argument and runs the software simulation:

```bash
./vadd
```

For reproducible output when debugging ordering-sensitive behavior, pin the simulation to a single thread:

```bash
TAPA_CONCURRENCY=1 ./vadd
```

```admonish note
`TAPA_CONCURRENCY` defaults to the physical CPU core count. Set it to `1` for reproducible task scheduling at the cost of simulation speed.
```

## Expected output

```
I20000101 00:00:00.000000 0000000 task.h:66] running software simulation with TAPA library
kernel time: 1.19429 s
PASS!
```

The log line confirms the software simulation path was taken. `PASS!` is printed by the application when its correctness check succeeds.

## Stream logging

To capture the values flowing through every `tapa::stream` channel, set `TAPA_STREAM_LOG_DIR` before running:

```bash
TAPA_STREAM_LOG_DIR=/tmp/logs ./vadd
```

TAPA writes one log file per stream. The format depends on the element type:

- **Primitive types** (`int`, `float`, …) are logged as human-readable text, one value per line. For example, writing `42` to a `tapa::stream<int>` produces `42\n`.
- **Non-primitive types without `operator<<`** are logged in hex with little-endian byte order. For example, writing `Foo{0x4222}` to a `tapa::stream<Foo>` produces `0x22420000\n`.
- **Non-primitive types with `operator<<` defined** are logged using that operator, producing human-readable text.

## Debugging with GDB

Software simulation runs as ordinary host code, so GDB works as normal:

```bash
gdb ./vadd
```

Then set a breakpoint on any TAPA task function by name:

```gdb
(gdb) b VecAdd
(gdb) run
```

Breakpoints, watchpoints, and backtraces all work because every task runs as a coroutine on the host CPU.

## Validation

Simulation is correct when:

1. The program exits with code 0.
2. The application's own correctness check prints `PASS!` (or your application's equivalent).
3. No deadlock or hang occurs within the expected runtime.

## If something goes wrong

```admonish warning
If the simulation hangs indefinitely, a stream deadlock is likely. See [Deadlocks & Hangs](../troubleshoot/deadlocks-and-hangs.md) for diagnosis steps.

For unexpected errors or assertion failures, see [Common Errors](../troubleshoot/common-errors.md).
```

---

**Next step:** [Fast Hardware Simulation](fast-cosim.md)
