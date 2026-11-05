# Memory Access: async_mmap

**Purpose:** Use async_mmap to overlap DRAM access latency with computation.

**Prerequisites:** [Memory Access: mmap](mmap.md)

---

## Why this exists

`mmap` leaves memory transaction scheduling to Vitis HLS. HLS can pipeline
and burst suitable access patterns, but `mmap` does not expose explicit
request and response channels for you to schedule yourself.

`async_mmap` exposes the five AXI channels as individual streams. You can keep
multiple requests outstanding and overlap address issuance with data receipt,
which is useful for random access and other latency-sensitive patterns. As with
any interface change, measure throughput and resource usage on your own design.

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
void ReadKernel(tapa::async_mmap<float>& mem, tapa::mmap<float> result,
                uint64_t n) {
  [[tapa::pipeline(1)]]
  for (uint64_t i_req = 0, i_resp = 0; i_resp < n;) {
    if (i_req < n && mem.read_addr.try_write(i_req)) {
      ++i_req;
    }
    float value;
    if (mem.read_data.try_read(value)) {
      result[i_resp] = value;
      ++i_resp;
    }
  }
}
```

Two loop counters (`i_req`, `i_resp`) track outstanding requests. Because both
checks are non-blocking, the loop can issue a new address and receive a
response in the same clock cycle.

---

## Writes must drain `write_resp`

Writes coordinate three channels: `write_addr`, `write_data`, and
`write_resp`. Extend the two-counter loop so issue and drain both progress
every cycle:

```cpp
void WriteKernel(tapa::async_mmap<float>& mem,
                 tapa::istream<float>& data, uint64_t n) {
  [[tapa::pipeline(1)]]
  for (uint64_t i_req = 0, i_resp = 0; i_resp < n;) {
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

Before issuing a write, all three preconditions must hold: the input stream
must have data, and neither the address nor the data channel may be full.
Checking them together prevents partial commits. `write_resp` must be drained
even when the count is unused: once its FIFO fills, the hardware stops
accepting new write addresses and the kernel deadlocks. Splitting writes and
response drain into separate loops risks the same deadlock if the FIFO fills
before all writes are issued. Because `ack` encodes `burst_length - 1`, one
response can acknowledge a whole coalesced burst, hence
`i_resp += unsigned(ack) + 1`.

---

## Sequential accesses and bursts

TAPA can coalesce eligible sequential element addresses into AXI bursts.
You issue element indices; adjacent requests are merged when AXI constraints
allow it. This provides burst efficiency for sequential patterns without
explicit burst programming in your kernel code.

---

## Rules

- `async_mmap<T>` must be passed **by reference** (`async_mmap<T>&`). Passing
  by value is a compile error.
- Channel operations (`try_read`/`try_write` on the five streams) are
  **leaf-task only**. An upper-level task may accept and forward an
  `async_mmap<T>&` parameter to a child leaf task without operating on it.
- An `mmap<T>` argument can be passed to an `async_mmap<T>&` parameter — mmap
  is automatically promoted.
- In the two-counter pipelined pattern, use non-blocking `try_read` and
  `try_write` on the five channels so a stall on one channel does not prevent
  progress on another.
- Drain `write_resp` while writes are in flight. Its value encodes
  `burst_length - 1`, so one response can complete multiple writes.

```admonish warning
A blocking operation on an `async_mmap` channel inside a pipelined loop can
prevent the other channels from making progress and can deadlock the task.
Use the non-blocking pattern unless you have analyzed the channel dependencies.
```

---

## Common mistakes

**Wrong** — `async_mmap` passed by value (compile error):

```cpp
void Kernel(tapa::async_mmap<float> mem) { /* ... */ }  // missing &
```

**Right** — `async_mmap` passed by reference:

```cpp
void Kernel(tapa::async_mmap<float>& mem) { /* ... */ }
```

**Risky** — blocking read inside the two-counter pipelined loop:

```cpp
// Can prevent further address issuance while no data is available.
float val = mem.read_data.read();
```

**Preferred** — non-blocking read with availability check:

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
