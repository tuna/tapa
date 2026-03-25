# Memory Access: mmap

**Purpose:** Access FPGA-adjacent DRAM from TAPA leaf tasks using mmap.

**Prerequisites:** [Tasks](tasks.md)

---

## Why this exists

FPGA designs need to read from and write to off-chip DRAM. `tapa::mmap<T>`
provides an array-like interface that TAPA compiles to AXI4 memory-mapped
transactions. It is simpler to use than `async_mmap` and is the right choice
when latency hiding is not required or when access patterns are straightforward.

---

## Mental model

A leaf task receives `mmap<T>` by value and accesses it like a C array:

```cpp
void Mmap2Stream(tapa::mmap<const float> mem, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream << mem[i];   // array subscript operator
  }
}
```

The upper-level task passes the mmap argument through to the leaf:

```cpp
void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a");
  tapa::stream<float> b_q("b");
  tapa::stream<float> c_q("c");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
```

---

## Minimal correct example

`Mmap2Stream` from the vector-add example reads from a read-only mmap and
writes the values into a stream:

```cpp
void Mmap2Stream(tapa::mmap<const float> mmap, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream << mmap[i];
  }
}
```

Note that `mmap` is passed by value (no `&`).

---

## Host-side wrappers

On the host, the direction of host-to-kernel data transfer is declared in the
`tapa::invoke` call using wrapper types:

- `tapa::read_only_mmap<T>(vec)` — host sends data to the kernel; kernel reads
- `tapa::write_only_mmap<T>(vec)` — kernel writes; host receives data back
- `tapa::read_write_mmap<T>(vec)` — bidirectional transfer

```admonish warning
`read_only_mmap` and `write_only_mmap` describe the **host-to-kernel transfer
direction**, not the kernel's internal access pattern. The kernel task always
receives a plain `mmap<T>` parameter regardless of which wrapper was used.
```

From the vector-add host code:

```cpp
tapa::invoke(
    VecAdd, FLAGS_bitstream,
    tapa::read_only_mmap<const float>(a),
    tapa::read_only_mmap<const float>(b),
    tapa::write_only_mmap<float>(c),
    n);
```

---

## Aligned allocator

If the host `std::vector` is not page-aligned, the TAPA runtime must make an
extra copy when transferring data to the FPGA. Use `tapa::aligned_allocator<T>`
to avoid this:

```cpp
std::vector<float, tapa::aligned_allocator<float>> a(n);
std::vector<float, tapa::aligned_allocator<float>> b(n);
std::vector<float, tapa::aligned_allocator<float>> c(n);
```

This eliminates the extra copy and suppresses XRT alignment warnings.

---

## Shared mmap

The same `mmap` argument can be passed to multiple child tasks. TAPA inserts
an AXI interconnect so both tasks share the same AXI port:

```cpp
void Load(tapa::mmap<float> srcs, uint64_t n,
          tapa::ostream<float>& a, tapa::ostream<float>& b) {
  tapa::task()
      .invoke(Mmap2Stream, srcs, 0, n, a)
      .invoke(Mmap2Stream, srcs, 1, n, b);
}
```

```admonish warning
When a mmap is shared across tasks, the programmer is responsible for memory
consistency. Concurrent accesses to the same addresses will produce undefined
results.
```

---

## mmap arrays

For parameterized designs with multiple independent memory ports:

- `tapa::mmaps<T, N>` — array of N mmap interfaces (kernel side)
- `tapa::read_only_mmaps<T, N>` / `tapa::write_only_mmaps<T, N>` /
  `tapa::read_write_mmaps<T, N>` — directional wrappers for `tapa::invoke` on
  the host side

```cpp
// Host side
tapa::invoke(VecAdd, FLAGS_bitstream,
             tapa::read_only_mmaps<float, M>(a),
             tapa::read_only_mmaps<float, M>(b),
             tapa::write_only_mmaps<float, M>(c), n);

// Kernel side
void VecAdd(tapa::mmaps<float, M> a, tapa::mmaps<float, M> b,
            tapa::mmaps<float, M> c, uint64_t n) { /* ... */ }
```

---

## Rules

- Kernel task signatures: `mmap<T>` must be passed **by value** (no `&`). This
  is the opposite of streams.
- `mmap` can only be used as a function parameter, not as a local variable.
- `read_only_mmap` / `write_only_mmap` describe host-to-kernel transfer
  direction only; they do not constrain kernel access patterns.

---

## Common mistakes

**Wrong** — mmap passed by reference:

```cpp
void Kernel(tapa::mmap<float>& mem) { /* ... */ }  // & is wrong
```

**Right** — mmap passed by value:

```cpp
void Kernel(tapa::mmap<float> mem) { /* ... */ }
```

---

## See also

- [Memory Access: async_mmap](async-mmap.md)
- [Performance Tuning](../howto/performance-tuning.md)

---

**Next step:** [Memory Access: async_mmap](async-mmap.md)
