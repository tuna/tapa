# Deadlocks & Hangs

**When to use this page:** When software simulation or fast cosim hangs without producing output, or terminates without printing results.

---

## Diagnosis checklist

Work through the following causes in order — they are listed from most to least common.

### 1. Stream depth too shallow

A producer fills the FIFO and blocks waiting for the consumer to drain it. If the consumer is itself waiting for data from another stream, neither task can make progress and the simulation hangs.

**Fix:** Increase the stream depth by providing the second template argument.

```cpp
// Default depth of 2 — may deadlock under backpressure
tapa::stream<int> s("s");

// Larger depth gives the producer room to run ahead
tapa::stream<int, 32> s("s");
```

Start at the default depth of 2 and increase to 16 or 32 when you observe backpressure. In hardware, deeper FIFOs consume more BRAM, so avoid over-provisioning depth once correctness is confirmed.

### 2. Missing loop termination or element count mismatch

A writer sends fewer elements than the reader expects. The reader blocks indefinitely waiting for data that never arrives.

**Fix:** Verify that every producer sends exactly as many elements as the corresponding consumer reads. A common mistake is an off-by-one in loop bounds or a conditional `write` that skips elements.

### 3. Circular dependency between tasks

Task A waits for output from Task B before it can write to Task B's input. Task B waits for input from Task A before it can produce output. Neither can make progress.

**Fix:** Redesign the data flow to eliminate the cycle. If a feedback path is genuinely required, use `try_read` / `try_write` so that a task can make progress even when the channel is empty or full.

### 4. `async_mmap` write responses not drained

The `write_resp` FIFO fills up. Once full, the hardware stops accepting new write addresses and the kernel stalls.

**Fix:** Always drain `write_resp` inside the write loop. A typical pattern:

```cpp
void WriteTask(tapa::mmap<int> mem, tapa::async_mmap<int>& async_mem, int n) {
  for (int i = 0; i < n; ++i) {
    // issue write address and data
    async_mem.write_addr.write(i);
    async_mem.write_data.write(mem[i]);
  }
  // drain all responses
  for (int i = 0; i < n; ) {
    if (!async_mem.write_resp.empty()) {
      async_mem.write_resp.read(nullptr);
      ++i;
    }
  }
}
```

---

## Isolation strategy

Run with `TAPA_CONCURRENCY=1` to serialize all tasks into a single coroutine thread. This makes a hang deterministic and easier to reproduce and attach a debugger to.

```bash
TAPA_CONCURRENCY=1 ./vadd
```

If the hang disappears at concurrency 1 but reappears at the default concurrency, the issue is a scheduling race rather than a structural deadlock. Look for assumptions about task ordering that do not hold under concurrent scheduling.

---

## Finding the blocked task

Attach GDB to the hung process to identify which task is stuck and on which operation.

```bash
gdb ./vadd
```

Let the binary run until it hangs, then interrupt it:

```
^C
(gdb) info threads
(gdb) thread apply all bt
```

The backtrace will show the call stack for every coroutine. Look for a frame inside a `read` or `write` call on a TAPA stream — the stream name in that frame identifies where flow has stopped.

---

## Waveform debugging in fast cosim

Run cosim with a persistent work directory and waveform capture enabled so you can inspect the simulation state after a hang.

```bash
./vadd --bitstream=vadd.xo \
  -xosim_work_dir ./cosim_work \
  -xosim_save_waveform \
  1000
```

If the simulation hangs, press Ctrl-C to terminate it, then open the waveform in Vivado:

```bash
vivado -mode gui -source ./cosim_work/output/run/run_cosim.tcl
```

Inspect the AXI and stream signals to identify which channel is stalled. A valid signal held high with a ready signal held low indicates backpressure; a ready signal high with no valid indicates the producer has stopped sending.

---

```admonish tip
Set `TAPA_STREAM_LOG_DIR=/tmp/stream_logs` before running to log all stream transfers to files in that directory:

```bash
TAPA_STREAM_LOG_DIR=/tmp/stream_logs ./vadd
```

Each stream gets its own log file. A stream with an empty or truncated log identifies where data flow stops.
```

---

## Stream depth tuning reference

| Symptom | Starting depth | Suggested increase |
|---------|---------------|-------------------|
| Hang with 2 tasks in a pipeline | 2 (default) | 16 |
| Hang with deep pipeline (>4 stages) | 16 | 32–64 |
| Correctness issue, no hang | Any | Try 2 first to expose races |

Increasing depth lets producers run further ahead of consumers and resolves backpressure-induced deadlocks. In hardware, each entry in a stream FIFO consumes flip-flops or BRAM. Once the design is functionally correct, profile resource usage and reduce depths where headroom allows.

---

**See also:** [Common Errors](common-errors.md) | [Cosimulation Issues](cosim-issues.md)
