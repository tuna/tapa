# Memory Access: mmap

**Purpose:** Access FPGA-adjacent DRAM from TAPA leaf tasks using mmap.

**Prerequisites:** [Tasks](tasks.md)

---

## Why this exists

FPGA designs need to read from and write to off-chip DRAM. `tapa::mmap<T>`
provides an array-like interface that TAPA compiles to AXI4 memory-mapped
transactions. It is simpler to use than `async_mmap` and is a good choice
when you do not need to control outstanding requests explicitly.

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

## Host-side wrappers

On the host, wrappers in the `tapa::invoke` call describe what the kernel does
with each buffer and therefore which transfers the runtime performs:

| Wrapper | Kernel view | Host-to-device before execution | Device-to-host after execution |
|---------|-------------|---------------------------------|--------------------------------|
| `tapa::read_only_mmap<T>(vec)` | Reads the buffer | Yes | No |
| `tapa::write_only_mmap<T>(vec)` | Writes the buffer | No | Yes |
| `tapa::read_write_mmap<T>(vec)` | Reads and writes | Yes | Yes |
| `tapa::placeholder_mmap<T>(vec)` | Leaves the host's data unused | No | No |

The kernel task still receives a plain `mmap<T>` or `mmaps<T, N>` parameter.
These wrappers control runtime transfers; they do not add C++ access
restrictions beyond the element type's constness.

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

This allows XRT to use the allocation without an alignment-related copy.

---

## Shared mmap

The same `mmap` argument can be passed to multiple child tasks. Give each
task a disjoint region or add synchronization when sharing. Here one buffer
is split into regions by an `offset` argument, and two instances of the same
leaf task read different regions of it:

```cpp
// Leaf task: reads the region starting at element n * offset.
void Mmap2Stream(tapa::mmap<float> mem, int offset, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream.write(mem[n * offset + i]);
  }
  stream.close();
}

void Load(tapa::mmap<float> srcs, uint64_t n,
          tapa::ostream<float>& a, tapa::ostream<float>& b) {
  tapa::task()
      .invoke(Mmap2Stream, srcs, 0, n, a)
      .invoke(Mmap2Stream, srcs, 1, n, b);
}
```

The complete working version of this is `tests/functional/shared-mmap/` in the
repository.

```admonish warning
When a mmap is shared across tasks, the programmer is responsible for memory
consistency. Unordered accesses to the same addresses can produce inconsistent
results. Partition the buffer or add an explicit synchronization protocol.
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

Only an upper-level task takes `mmaps`: each task it invokes with the array
takes the next region as its `mmap<T>`. `tapa synth` rejects a leaf task's
`mmaps` parameter.

---

## Rules

- Kernel task signatures: `mmap<T>` must be passed **by value** (no `&`);
  passing by reference is a compile error. This is the opposite of streams.
- Within a kernel task body, `mmap` is a function-parameter interface, not a
  local variable to allocate.
- Host wrappers describe the kernel's buffer access and select the required
  host/device transfers; they do not change the kernel parameter types.
- The kernel assumes each `mmap` starts on a 64-byte boundary, so HLS reads
  a wide element, such as a 512-bit `tapa::vec_t<float, 16>`, as one bus word
  without realigning it. The TAPA runtime, XRT and the cosimulators place
  every buffer on such a boundary; a system that programs a kernel's
  addresses itself must do the same.

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
