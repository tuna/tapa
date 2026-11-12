# Lab 2: High-Bandwidth Memory with async_mmap

**Goal:** Achieve high DRAM throughput by overlapping multiple outstanding memory requests using `async_mmap`.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Memory Access: async_mmap](../concepts/async-mmap.md)

After this lab you will understand:
- When explicit request scheduling beats leaving it to HLS
- Where the two-counter read and write patterns are documented, and the
  mistakes they prevent

---

## When explicit request scheduling matters

With `mmap<T>`, Vitis HLS schedules memory accesses for you: it may pipeline
and burst suitable patterns, but you cannot directly issue an address and
then continue independent work while waiting for its response. `async_mmap`
exposes the five AXI channels as streams so you can — especially useful for
random access or custom scheduling. Why the interface exists, its channel
layout, and its usage rules are covered by the concept page this lab walks
through.

---

## The two-counter read pattern

The core skill is a single pipelined loop with two counters — one for
addresses issued, one for responses collected — so the loop can issue a new
address and receive a response in the same clock cycle. The pattern, its
write-side counterpart, and burst requests are all worked through in
[Memory Access: async_mmap](../concepts/async-mmap.md); read the
[minimal correct example](../concepts/async-mmap.md#minimal-correct-example)
and [writes must drain `write_resp`](../concepts/async-mmap.md#writes-must-drain-write_resp)
there before continuing. Two points from the write pattern are worth
restating, because they are the classic mistakes: `write_resp` must be
drained even when the count is unused — a full `write_resp` FIFO deadlocks
the kernel — and each `ack` encodes `burst_length - 1`, so one response can
acknowledge a whole coalesced burst.

For a complete working design that drives `async_mmap` over many memory
channels, see `tests/apps/bandwidth` in the repository (see the
[Examples Catalog](examples-catalog.md)).

---

**Next step:** [Lab 3: Migrating from Vitis HLS](lab-03-vitis-hls.md)
