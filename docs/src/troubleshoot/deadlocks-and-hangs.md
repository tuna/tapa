# Deadlocks & Hangs

**When to use this page:** When software simulation or fast cosim hangs without producing output, or terminates without printing results.

```admonish note
`tapa::stream` enforces the declared depth in both software simulation and fast cosim/RTL. A blocking `write()` on a full stream yields the current coroutine and retries until space is available — so shallow stream depth can deadlock in software simulation too. The exception is `tapa::hls::stream` (the Vitis HLS compatibility alias), which uses effectively infinite depth in software simulation.
```

---

## Start with the stall warning

Software simulation names the stuck channel for you. After a task has been
blocked on one channel for 10 seconds with nothing it polls clearing, TAPA
logs:

```text
W tapa: no stream progress for 10s; blocked on channel 'fifo_A' (empty). ...
```

`empty` means a consumer is starved, `full` means a producer is backed up —
which is usually enough to identify the pair below. Lower the threshold with
`TAPA_STALL_WARN_SECONDS=2` to see it sooner, or set `0` to silence it. The
warning stays quiet while a kernel instance is running, because waiting on
hardware or RTL is not a stall.

Fast cosim has no equivalent: use `-cosim_timeout_seconds` (see
[Cosim Issues](./cosim-issues.md)) and reproduce in software simulation first.

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

A writer and reader disagree about the number of elements in a transaction. If
the writer sends too few, the reader waits forever; if it sends too many, the
writer eventually blocks on a full FIFO after the reader stops.

**Fix:** Verify that every producer sends exactly as many elements as the
corresponding consumer reads. Common mistakes include an off-by-one loop bound,
a conditional `write` that skips elements, and padding a fixed-size memory
buffer without updating the protocol metadata that controls downstream reads.

This bites hardest in a *fixed-count* architecture, where a producer's trip
count is a compile-time constant but its consumer's comes from host data:

```cpp
constexpr int kBeatsPerChannel = 1139;  // baked into the RTL

void ReadA(tapa::async_mmap<tapa::u<512>>& a, tapa::ostream<tapa::u<512>>& out) {
  for (int req = 0, resp = 0; resp < kBeatsPerChannel;) {
    // async_read() here is an application helper (see the regression
    // designs), not a TAPA API: it issues the request and parks the beat.
    async_read(a, out, kBeatsPerChannel, req, resp);
  }
}

void Compute(tapa::istream<tapa::u<512>>& in, tapa::istream<int>& config) {
  const int beats = config.read();  // whatever the host wrote
  for (int i = 0; i < beats; ++i) in.read();
}
```

Run a small input through it and `ReadA` writes 1139 beats while `Compute`
takes only as many as the host's pointer claims. The FIFO fills, `ReadA`
blocks forever, and the stall warning names its output channel as `full`.

**Fix:** pad the payload *and* the count/end pointer to the fixed size, before
copying either into the mmap. Resizing only the host allocation leaves the
pointer saying "5 beats" while the RTL still emits 1139.

### 3. Circular dependency between tasks

Task A waits for output from Task B before it can write to Task B's input. Task B waits for input from Task A before it can produce output. Neither can make progress.

**Fix:** Redesign the data flow to eliminate the cycle. If a feedback path is genuinely required, use `try_read` / `try_write` so that a task can make progress even when the channel is empty or full.

### 4. `async_mmap` write responses not drained

The `write_resp` FIFO fills up. Once full, the hardware stops accepting new write addresses and the kernel stalls.

**Fix:** Always drain `write_resp` inside the same pipelined loop that issues writes. Use non-blocking `try_write` / `try_read` so both issue and drain progress every cycle:

```cpp
void WriteTask(tapa::async_mmap<int>& mem, tapa::istream<int>& data, int n) {
  [[tapa::pipeline(1)]] for (int64_t i_req = 0, i_resp = 0; i_resp < n;) {
    if (i_req < n && !data.empty() &&
        !mem.write_addr.full() && !mem.write_data.full()) {
      mem.write_addr.try_write(i_req);
      mem.write_data.try_write(data.read(nullptr));
      ++i_req;
    }
    uint8_t ack;
    if (mem.write_resp.try_read(ack)) {
      i_resp += unsigned(ack) + 1;  // ack encodes burst_length - 1
    }
  }
}
```

Splitting writes and response drain into separate loops risks deadlock: if `write_resp` fills before all writes are issued, the hardware stops accepting write addresses and the first loop never completes.

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
  -cosim_work_dir ./cosim_work \
  -xsim_save_waveform \
  1000
```

If the simulation hangs, press Ctrl-C to terminate it, then open the waveform in Vivado:

```bash
vivado -mode gui -source ./cosim_work/run_cosim.tcl
```

Inspect the AXI and stream signals to identify which channel is stalled. A valid signal held high with a ready signal held low indicates backpressure; a ready signal high with no valid indicates the producer has stopped sending.

---

```admonish tip
Set `TAPA_STREAM_LOG_DIR=/tmp/stream_logs` before running. TAPA logs each value written to a stream into a file under that directory:

    TAPA_STREAM_LOG_DIR=/tmp/stream_logs ./vadd

Each named stream gets its own log file. A stream with an empty or truncated log identifies where data flow stops.
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
