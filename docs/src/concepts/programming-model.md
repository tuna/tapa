# The Programming Model

**Purpose:** Understand the TAPA task-parallel programming model.

**Prerequisites:** [Installation](../start/installation.md)

TAPA bridges familiar sequential C++ to FPGA hardware parallelism. Rather than
requiring users to write RTL directly, it lets them express computation as a
graph of concurrently-running tasks communicating through typed streams and
shared memory interfaces.

---

## Why this exists

Writing FPGA accelerators traditionally requires either low-level RTL or
fragile HLS pragmas that break when code is refactored. TAPA solves this by
letting you describe the parallel structure of your design as a graph of C++
functions. The compiler turns that graph into RTL automatically, while the same
code runs natively on a CPU for simulation. You get the productivity of C++
without giving up the ability to express fine-grained, concurrent hardware
pipelines.

---

## Mental model

A TAPA design is a directed graph of tasks connected by streams and memory
interfaces. Scalars are passed as function arguments.

```
Host
 │  tapa::invoke(TopTask, bitstream, mmap_args...)
 ▼
Top-level task  ← no computation; spawns all leaf tasks
 ├── spawns ──> Leaf task A  (writes to stream S)
 │                            stream S
 ├── spawns ──> Leaf task B  (reads stream S, writes to stream T)
 │                            stream T
 └── spawns ──> Leaf task C  (reads stream T, writes to mmap)
                              mmap ──> DRAM
```

- The **host** calls `tapa::invoke`, passing the kernel function, a bitstream
  path (empty for software simulation), and the kernel arguments.
- The **top-level task** is the entry point synthesized by `tapa compile`. It
  declares streams as local objects, then spawns all leaf tasks and passes
  streams to them by reference. It contains no computation of its own.
- **Leaf tasks** perform the actual computation. One leaf writes to a stream;
  another reads from it. Streams flow *between* leaf tasks — the top-level task
  is never the producer or consumer of stream data.

All child tasks spawned by `tapa::task().invoke(...)` run **concurrently**. The
top-level task returns only after every child task has finished.

---

## Minimal correct example

### Kernel file (`vadd.cpp`)

The top-level task `VecAdd` declares three streams, then launches four leaf
tasks that run in parallel:

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

### Host file (`vadd-host.cpp`)

The host calls `tapa::invoke` with the kernel function, the bitstream path, and
the kernel arguments. When the bitstream path is empty (the default), TAPA runs
software simulation:

```cpp
#include <gflags/gflags.h>
#include <tapa.h>

DEFINE_string(bitstream, "", "Path to XO or xclbin file. Empty = software simulation.");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);

  std::vector<float, tapa::aligned_allocator<float>> a(n), b(n), c(n);
  // ... fill a and b ...

  int64_t kernel_time_ns = tapa::invoke(
      VecAdd, FLAGS_bitstream,
      tapa::read_only_mmap<const float>(a),
      tapa::read_only_mmap<const float>(b),
      tapa::write_only_mmap<float>(c),
      n);
}
```

The `--bitstream` flag is what controls which backend runs:
- Omitted or empty → software simulation
- `.xo` → fast cosimulation
- `.hw.xclbin` → on-board execution

---

## Rules

- Host code and kernel code **must live in separate files**. The kernel file is
  compiled to RTL; the host file is compiled to a CPU executable.
- The kernel file must contain **exactly one top-level task** — the function
  passed as `--top` to `tapa compile`.
- The top-level task is **called via `tapa::invoke` from the host**; never
  called directly.
- An upper-level task body **must contain only** stream declarations,
  `tapa::task().invoke(...)` chains, and scalar/mmap argument forwarding — no
  computation.
- Streams are passed **by reference** (`tapa::istream<T>&`,
  `tapa::ostream<T>&`). Passing streams by value is a compile error.
- mmap arguments are passed **by value** (`tapa::mmap<T>`), not by reference.
- Scalar arguments (plain C++ types such as `int`, `float`, `uint64_t`) are passed
  by value and are **read-only to the kernel**. The kernel cannot communicate a
  result back to the host through a scalar parameter; use an mmap or stream instead.
- Software simulation runs automatically when `tapa::invoke` receives an empty
  bitstream path.

---

## Common mistakes

### Wrong: calling the top-level task directly from host code

```cpp
// WRONG — bypasses the TAPA runtime entirely; streams are not initialized,
// hardware execution cannot be dispatched.
VecAdd(tapa::mmap<const float>(a.data()), /* ... */);
```

### Right: always use `tapa::invoke`

```cpp
// RIGHT — works for software simulation, cosim, and on-board execution.
tapa::invoke(VecAdd, FLAGS_bitstream,
             tapa::read_only_mmap<const float>(a),
             tapa::read_only_mmap<const float>(b),
             tapa::write_only_mmap<float>(c),
             n);
```

`tapa::invoke` examines the bitstream path at runtime and dispatches to the
correct backend: software simulation (empty path), RTL co-simulation (`.xo`),
emulation (`.hw_emu.xclbin`), or on-board execution (`.hw.xclbin`).

---

## See also

- [Tasks](tasks.md)
- [Streams](streams.md)
- [Memory Access: mmap](mmap.md)
- [Software Simulation](../howto/software-simulation.md)

**Next step:** [The Compile Pipeline](compile-pipeline.md)
