# Lab 2: High-Bandwidth Memory with async_mmap

**Goal:** Achieve high DRAM throughput by overlapping multiple outstanding memory requests using `async_mmap`.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Memory Access: async_mmap](../concepts/async-mmap.md)

After this lab you will understand:
- Why sequential memory access wastes most of the available DRAM bandwidth
- How the two-counter loop pattern keeps multiple requests in flight simultaneously
- How to correctly coordinate the three write channels and drain `write_resp`

---

## When explicit request scheduling matters

With `mmap<T>`, Vitis HLS schedules memory accesses for you. It may pipeline
and burst suitable patterns, but you cannot directly issue an address and
then continue independent work while waiting for its response.

`async_mmap` exposes the five AXI channels as streams. You can issue multiple
read addresses before consuming the responses, keeping requests in flight and
hiding memory latency. This is especially useful for random access or custom
scheduling. It does not guarantee higher throughput for every design, so
compare synthesis reports and measurements. See
[Memory Access: async_mmap](../concepts/async-mmap.md) for the channel layout
and usage rules.

---

## Example 1: Overlapping reads with a single loop

The idiomatic TAPA read pattern uses two counters in a single pipelined loop:

```cpp
void ReadKernel(tapa::async_mmap<float>& mem, tapa::mmap<float> result,
                uint64_t n) {
  [[tapa::pipeline(1)]]
  for (uint64_t i_req = 0, i_resp = 0; i_resp < n;) {
    if (i_req < n && mem.read_addr.try_write(i_req)) ++i_req;
    float val;
    if (mem.read_data.try_read(val)) {
      result[i_resp] = val;
      ++i_resp;
    }
  }
}
```

How it works:

- `i_req` tracks how many addresses have been issued; `i_resp` tracks how many responses have been received.
- The loop condition is `i_resp < n`: it runs until every response is collected, not just until every address is sent.
- `mem.read_addr.try_write(i_req)` is non-blocking. If the address channel is full this cycle, it returns false and the address is retried on the next cycle. `i_req` only advances when the write succeeds.
- `mem.read_data.try_read(val)` is non-blocking. If no data has arrived yet, it returns false and the loop continues without blocking.
- Because both branches are independent and non-blocking, the loop can issue a new address and receive a response **in the same clock cycle**.
- The difference `i_req - i_resp` is the current number of in-flight requests. The hardware limits this to the channel depth; TAPA coalesces sequential addresses into AXI bursts automatically at runtime, so you never need to write explicit burst logic.

---

## Example 2: Sequential writes with burst detection

Writes coordinate three channels: `write_addr`, `write_data`, and
`write_resp`. Use the [write pattern with response
drain](../concepts/async-mmap.md#writes-must-drain-write_resp) from the
concept page. Two points matter here: `write_resp` must be drained even when
the count is unused — a full `write_resp` FIFO deadlocks the kernel — and
each `ack` encodes `burst_length - 1`, so one response can acknowledge a
whole coalesced burst.

---

## Rules for using async_mmap

- Pass `async_mmap<T>` **by reference** (`async_mmap<T>&`). Passing by value is an error.
- In the two-counter pipelined pattern, use `try_read`/`try_write` on the
  `async_mmap` channels so one blocked channel does not prevent the others
  from progressing.
- Always drain `write_resp`, even if you discard the burst-length value.
- An `mmap<T>` argument can be passed to an `async_mmap<T>&` parameter in a child task without changing the caller.

```admonish warning
A blocking `read`/`write` on an `async_mmap` channel inside this pipelined
pattern can prevent progress on the other channels and deadlock the kernel.
Use the non-blocking operations unless you have analyzed the dependencies.
```

```admonish tip
For the full API reference and channel semantics, see
[Memory Access: async_mmap](../concepts/async-mmap.md).
```

---

**Next step:** [Lab 3: Migrating from Vitis HLS](lab-03-vitis-hls.md)
