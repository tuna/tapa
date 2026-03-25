# Lab 2: High-Bandwidth Memory with async_mmap

**Goal:** Achieve high DRAM throughput by overlapping multiple outstanding memory requests using `async_mmap`.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Memory Access: async_mmap](../concepts/async-mmap.md)

After this lab you will understand:
- Why sequential memory access wastes most of the available DRAM bandwidth
- How the two-counter loop pattern keeps multiple requests in flight simultaneously
- How to correctly coordinate the three write channels and drain `write_resp`

---

## The problem: one request at a time

With a plain `mmap<T>` argument, each read or write is a blocking operation. The loop below looks innocuous, but every iteration stalls waiting for data to return from DRAM before the next address is issued:

```cpp
// Problematic: one outstanding request at a time
for (int i = 0; i < n; i++) {
  result[i] = mem[i];  // blocks until data returns
}
```

Off-chip DRAM latency is typically 100–200 ns. At a 300 MHz clock that is 30–60 idle cycles per element. For sequential access patterns the HLS tool's burst inference may help, but for random-access patterns or when you need explicit control over request depth, `mmap` leaves most of the available bandwidth unused.

`async_mmap` solves this by exposing the five AXI channels directly as streams. You can issue many read addresses before any data returns, keeping dozens of requests in flight and hiding the per-request latency behind the steady flow of data. See [Memory Access: async_mmap](../concepts/async-mmap.md) for the channel layout and area comparison.

---

## Example 1: Overlapping reads with a single loop

The idiomatic TAPA read pattern uses two counters in a single pipelined loop:

```cpp
void ReadKernel(tapa::async_mmap<float>& mem, float* result, uint64_t n) {
  for (int i_req = 0, i_resp = 0; i_resp < n;) {
#pragma HLS pipeline II=1
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

Writes require coordinating three channels: `write_addr`, `write_data`, and `write_resp`. The pattern checks all three are ready before committing:

```cpp
void WriteKernel(tapa::async_mmap<float>& mem,
                 tapa::istream<float>& data, uint64_t n) {
  for (int i_req = 0, i_resp = 0; i_resp < n;) {
#pragma HLS pipeline II=1
    if (i_req < n && !data.empty() &&
        !mem.write_addr.full() && !mem.write_data.full()) {
      mem.write_addr.try_write(i_req);
      mem.write_data.try_write(data.read(nullptr));
      ++i_req;
    }
    uint8_t ack;
    if (mem.write_resp.try_read(ack)) {
      i_resp += unsigned(ack) + 1;  // ack encodes burst length - 1
    }
  }
}
```

Key points:

- Before issuing a write, all three preconditions must hold: the input stream must have data, and neither the address nor the data channel may be full. Checking them together prevents partial commits.
- `write_resp` must be consumed even if you do not use the count. The hardware stops accepting new write addresses once the `write_resp` FIFO fills up, causing deadlock if the kernel never drains it.
- The `ack` value encodes `burst_length - 1`. TAPA detects that you are issuing sequential addresses and merges them into AXI bursts at runtime. A single `write_resp` entry can therefore acknowledge many writes, which is why `i_resp += unsigned(ack) + 1` rather than `i_resp += 1`.

---

## Rules for using async_mmap

- Pass `async_mmap<T>` **by reference** (`async_mmap<T>&`). Passing by value is an error.
- Only use `try_read`/`try_write` inside pipelined loops. Blocking `read`/`write` stalls the pipeline and will cause deadlock when combined with other non-blocking channels.
- Always drain `write_resp`, even if you discard the burst-length value.
- An `mmap<T>` argument can be passed to an `async_mmap<T>&` parameter in a child task without changing the caller.

```admonish warning
Never use blocking `read`/`write` on `async_mmap` channels inside a pipelined loop. Because the five AXI channels are decoupled, blocking on one channel prevents progress on the others and causes the kernel to hang.
```

```admonish tip
For the full API reference and the area comparison table showing how `async_mmap` compares to the Vitis HLS `m_axi` interface, see [Memory Access: async_mmap](../concepts/async-mmap.md).
```

---

**Next step:** [Lab 3: Migrating from Vitis HLS](lab-03-vitis-hls.md)
