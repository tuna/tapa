# Your First Debug Cycle

Diagnose and fix failures in TAPA software simulation.

## Prerequisites

- TAPA installed — see [Installation](installation.md)
- A simulation binary built with `tapa g++` — see [Your First Run](first-run.md)

## Symptom

The simulation hangs without producing output, crashes with an error, or
prints `FAIL!` instead of `PASS!`.

## How to confirm: run single-threaded

By default TAPA runs each task in its own coroutine using a thread pool sized
to the number of physical CPU cores. Reducing concurrency to one thread improves
reproducibility and simplifies debugging:

```bash
TAPA_CONCURRENCY=1 ./vadd
```

If the hang disappears or a crash becomes reproducible, the problem is likely
a race condition or a deadlock that only manifests under concurrent execution.

## Fix patterns

### Attach GDB

Software simulation runs as a normal CPU process, so a debugger works without
any special setup:

```bash
gdb ./vadd
```

Set a breakpoint on any TAPA task function by name and run:

```
(gdb) b VecAdd
(gdb) run
```

You can set breakpoints on any leaf task (`Add`, `Mmap2Stream`, `Stream2Mmap`,
etc.) and step through the code exactly as you would for a regular C++
program.

### Dump stream contents

Set `TAPA_STREAM_LOG_DIR` to a directory path before running. TAPA will write
one log file per named stream under that directory, recording every value
written to the stream:

```bash
TAPA_STREAM_LOG_DIR=/tmp/logs ./vadd
```

Log format:

- **Primitive types** (`int`, `float`, …) are written as decimal text, one
  value per line.
- **Structs without `operator<<`** are written as little-endian hex.
- **Structs with `operator<<`** are written using your operator.

After the run, inspect the files under `/tmp/logs/` to trace data as it
flows through each stream and locate where incorrect values first appear.

## Common mistakes to check

| Symptom | Likely cause | Fix |
|---|---|---|
| Hangs forever | Deadlock or backpressure — a stream is full or empty and no task can make progress | [Deadlocks & Hangs](../troubleshoot/deadlocks-and-hangs.md) |
| Wrong output (`FAIL!`) | Logic error in a leaf task | Attach GDB or dump stream contents (above) |
| Build fails with template errors | Pass-by-value/reference mismatch on streams or mmaps | [Common Errors](../troubleshoot/common-errors.md) |

```admonish tip
Always pass your design through software simulation before attempting RTL
synthesis or hardware simulation. Software simulation compiles in seconds,
and standard tools like GDB and AddressSanitizer work without modification.

To catch memory errors, compile with sanitizers:

    tapa g++ -- vadd.cpp vadd-host.cpp -fsanitize=address -g -o vadd
```

## Next step

[Full FPGA Compilation](full-compilation.md)
