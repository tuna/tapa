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
... no stream progress for 10s; blocked on channel 'fifo_A' (empty). ...
```

`empty` means a consumer is starved, `full` means a producer is backed up —
which is usually enough to identify the pair below. Lower the threshold with
`TAPA_STALL_WARN_SECONDS=2` to see it sooner, or set `0` to silence it. The
warning stays quiet while a kernel instance is running, because waiting on
hardware or RTL is not a stall.

Fast cosim has no equivalent for the kernel's own channels; it reports only
the progress of streams bound to the host ([Stream
diagnostics](./cosim-issues.md#stream-diagnostics)). Use
`-cosim_timeout_seconds` (see [Cosim Issues](./cosim-issues.md)) and
reproduce in software simulation first.

## Diagnosis checklist

Work through the following causes, using the stall warning and a small input
to guide you.

### 1. A finite burst exceeds the FIFO capacity

Backpressure is normal: a full FIFO is supposed to stall its producer until
the consumer catches up. A shallow FIFO can, however, be too small for a
finite burst in an otherwise correct protocol. Increasing depth provides more
storage through the second template argument:

```cpp
tapa::stream<int> s("s");       // default depth: 2
tapa::stream<int, 32> s("s");  // more room for a bursty producer
```

Increasing depth cannot repair an element-count mismatch or a circular
protocol dependency; it only postpones backpressure. Check those causes
below before paying for a larger FIFO. Deeper FIFOs also consume more FPGA
resources, not necessarily BRAM—the implementation depends on depth and
width.

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

**Fix:** Redesign the protocol to guarantee progress. If a feedback path is
genuinely required, define its initial state, storage, and drain order
explicitly. Non-blocking `try_read`/`try_write` can be part of that design,
but they do not remove a logical cycle by themselves.

### 4. `async_mmap` write responses not drained

The `write_resp` FIFO fills up. Once full, the hardware stops accepting new write addresses and the kernel stalls.

**Fix:** Always drain `write_resp` inside the same pipelined loop that issues writes, using the non-blocking pattern from [Memory Access: async_mmap](../concepts/async-mmap.md#writes-must-drain-write_resp) so both issue and drain progress every cycle. Splitting writes and response drain into separate loops risks deadlock: if `write_resp` fills before all writes are issued, the hardware stops accepting write addresses and the first loop never completes.

---

## Isolation strategy

Run with `TAPA_CONCURRENCY=1` to use one coroutine worker. This simplifies
the scheduling interleaving and can make a hang easier to reproduce and debug;
tasks still interleave at blocking points rather than running to completion
one by one.

```bash
TAPA_CONCURRENCY=1 ./vadd
```

If the failure changes with worker count, look for ordering or timing
assumptions that are not enforced by stream or memory handshakes. This is
only a diagnostic clue, not proof of a scheduler bug or of a particular
deadlock cause.

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

Backtraces show the currently running coroutine on each worker thread;
suspended coroutines are not separate native threads. Look for frames in
TAPA stream reads, writes, or scheduler polling. Use the channel named by the
stall warning to identify the blocked endpoint when optimized frames do not
show it.

---

## Waveform debugging in fast cosim

Run cosim with a persistent work directory and waveform capture enabled so you can inspect the simulation state after a hang.

```bash
./vadd --bitstream=vadd.xo \
  -cosim_work_dir ./cosim_work \
  -xsim_save_waveform \
  1000
```

After stopping the simulation, open the saved `wave.wdb` in Vivado's waveform
viewer. Its default xsim location is
`./cosim_work/run/cosim.sim/sim_1/behav/xsim/wave.wdb`. Do not rerun
`run_cosim.tcl` just to inspect the old waveform; that starts a new
simulation.

Inspect AXI and stream handshakes to identify the stalled channel. Valid held
high with ready low indicates backpressure; ready high with valid low
indicates that the producer is not sending.

---

```admonish tip
Set `TAPA_STREAM_LOG_DIR` to an existing directory before running. TAPA logs
each value written to a named stream:

    mkdir -p /tmp/tapa-stream-logs
    TAPA_STREAM_LOG_DIR=/tmp/tapa-stream-logs ./vadd

Each named stream gets its own log file; unnamed streams are not logged.
Compare producer log counts with the consumer's expected counts to find where
data flow stops.
```

---

## Stream depth tuning reference

Start with the default depth of 2 to stress the handshake protocol, increase
it only for a measured burst or rate mismatch (see the stream-depth item in
[Performance Tuning](../howto/performance-tuning.md#prioritized-checklist)),
and re-run with a small depth after fixes to expose lingering count and
backpressure bugs.

---

**See also:** [Common Errors](common-errors.md) | [Cosimulation Issues](cosim-issues.md)
