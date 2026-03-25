# Memory Access: async_mmap

**Purpose:** Use async_mmap to overlap DRAM access latency with computation.

**Prerequisites:** [Memory Access: mmap](mmap.md)

---

## Why this exists

`mmap` does not provide explicit control over outstanding DRAM transactions.
The HLS tool may issue burst transactions for sequential access, but for
random-access patterns or designs that need fine-grained control over
outstanding requests, the lack of explicit flow control limits throughput.
Off-chip DRAM latency is typically 100–200 ns, and without the ability to
overlap request issuance with data receipt, achievable bandwidth stays far
below the channel peak.

`async_mmap` exposes the five AXI channels as individual streams, letting you
issue multiple outstanding requests and overlap address issuance with data
receipt. The result is much higher DRAM throughput — especially for random
access — and significantly lower area overhead compared to the Vitis HLS
`m_axi` interface.

---

## Mental model: five AXI channels

`async_mmap<T>` is a struct whose fields are streams corresponding to the five
AXI channels:

```cpp
template <typename T>
struct async_mmap {
  using addr_t = int64_t;
  using resp_t = uint8_t;

  tapa::ostream<addr_t> read_addr;   // issue read addresses
  tapa::istream<T>      read_data;   // receive read data
  tapa::ostream<addr_t> write_addr;  // issue write addresses
  tapa::ostream<T>      write_data;  // send write data
  tapa::istream<resp_t> write_resp;  // receive write acknowledgments
};
```

![async_mmap diagram](../figures/tapa-async-mmap.drawio.svg)

The key insight is that `read_addr` and `read_data` are decoupled: you can
issue many addresses into `read_addr` before any data arrives on `read_data`,
hiding latency by keeping multiple requests in flight simultaneously.

---

## Minimal correct example

The pattern for overlapping read requests and responses in a single pipelined
loop:

```cpp
void ReadKernel(tapa::async_mmap<float>& mem, float* result,
                uint64_t n) {
  for (int i_req = 0, i_resp = 0; i_resp < n;) {
#pragma HLS pipeline II=1
    // Issue a read address if the channel has space
    if (i_req < n && mem.read_addr.try_write(i_req)) {
      ++i_req;
    }
    // Consume a read response if data is available
    if (!mem.read_data.empty()) {
      result[i_resp] = mem.read_data.read(nullptr);
      ++i_resp;
    }
  }
}
```

Two loop counters (`i_req`, `i_resp`) track outstanding requests. Because both
checks are non-blocking, the loop can issue a new address and receive a
response in the same clock cycle.

---

## Runtime burst detection

TAPA coalesces sequential addresses into AXI bursts automatically at runtime.
You only need to issue individual element-by-element addresses; TAPA's
generated hardware merges adjacent requests into larger burst transactions
dynamically. This provides burst efficiency for sequential patterns without
requiring static analysis or explicit burst programming in your kernel code.

---

## Area comparison

`async_mmap` uses significantly fewer FPGA resources than the Vitis HLS
`m_axi` interface, which is important for HBM devices that expose many memory
channels:

| Memory Interface             | Clock (MHz) | LUT  | FF   | BRAM | URAM | DSP |
|------------------------------|-------------|------|------|------|------|-----|
| `#pragma HLS interface m_axi` | 300        | 1189 | 3740 | 15   | 0    | 0   |
| `async_mmap`                  | 300        | 1466 | 162  | 0    | 0    | 0   |

`async_mmap` uses no BRAM and drastically fewer flip-flops, at the cost of
slightly more LUTs for the burst-detection logic.

---

## Rules

- `async_mmap<T>` must be passed **by reference** (`async_mmap<T>&`). Passing
  by value is deprecated.
- Channel operations (`try_read`/`try_write` on the five streams) are
  **leaf-task only**. An upper-level task may accept and forward an
  `async_mmap<T>&` parameter to a child leaf task without operating on it.
- An `mmap<T>` argument can be passed to an `async_mmap<T>&` parameter — mmap
  is automatically promoted.
- Only **non-blocking** operations (`try_read`, `try_write`) should be used on
  `async_mmap` channels inside pipelined loops.

```admonish warning
Never use blocking `read`/`write` on `async_mmap` channels inside a pipelined
loop. Blocking operations prevent other channel progress and cause deadlock.
Always use `try_read` and `try_write`.
```

---

## Common mistakes

**Wrong** — `async_mmap` passed by value (deprecated):

```cpp
void Kernel(tapa::async_mmap<float> mem) { /* ... */ }  // missing &
```

**Right** — `async_mmap` passed by reference:

```cpp
void Kernel(tapa::async_mmap<float>& mem) { /* ... */ }
```

**Wrong** — blocking read inside a pipelined loop:

```cpp
// Wrong: blocks until data arrives, preventing address issuance
float val = mem.read_data.read();
```

**Right** — non-blocking read with availability check:

```cpp
float val;
if (mem.read_data.try_read(val)) {
  // process val
}
```

---

## See also

- [Lab 2: High-Bandwidth Memory](../tutorials/lab-02-async-mmap.md)
- [Performance Tuning](../howto/performance-tuning.md)
- [C++ API](../reference/api.md)

---

**Next step:** [Software Simulation](../howto/software-simulation.md)
