# The Programming Model

**Purpose:** Understand the TAPA task-parallel programming model.

**Prerequisites:** [Installation](../start/installation.md)

TAPA uses C++ to express a task-parallel dataflow design. You describe the
concurrency explicitly as a graph of tasks connected by typed streams and
memory interfaces; TAPA does not infer parallelism from a sequential program.

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
Top-level task  ← orchestrates child tasks
 ├── spawns ──> Leaf task A  (writes to stream S)
 │                            stream S
 ├── spawns ──> Leaf task B  (reads stream S, writes to stream T)
 │                            stream T
 └── spawns ──> Leaf task C  (reads stream T, writes to mmap)
                              mmap ──> DRAM
```

- The **host** calls `tapa::invoke`, passing the kernel function, a bitstream
  path (empty for software simulation), and the kernel arguments.
- The **top-level task** is the entry point selected by `tapa compile --top`.
  For the default `xilinx-vitis` target, it is an upper-level task that declares
  channels and launches children without doing computation itself.
- **Leaf tasks** perform the actual computation. One leaf writes to a stream;
  another reads from it. In this upper-level shape, streams flow between leaf
  tasks rather than through the orchestration body.

All child tasks spawned by `tapa::task().invoke(...)` run **concurrently**. The
top-level task returns only after every joined (not detached) child task has
finished.

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
#include <cstdint>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n);

DEFINE_string(bitstream, "", "Path to XO or xclbin file. Empty = software simulation.");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);

  const uint64_t n = 1024;
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
- `.xo` or `.zip` → fast cosimulation
- `.xclbin` → Vitis hardware emulation if the xclbin was built for `hw_emu`,
  on-board execution otherwise
- `.vbin` → an Alveo V80 through SLASH, or SLASH simulation for a vbin linked
  with `--sim` ([Alveo V80 with SLASH](../howto/v80.md))
- a package from `tapa compile --aie-sim` → an AI Engine simulator ([AI
  Engine Tasks](../howto/aie.md))

---

## Rules

- Keep host and kernel code in separate translation units in the normal flow.
  Pass only kernel sources to `tapa compile`; build the host with `tapa g++`.
  For larger programs, see [Multi-File Designs](../howto/multi-file.md).
- Each compilation selects exactly one top-level task with `--top`. The source
  may also contain the child tasks and unrelated helper functions.
- The top-level task is **called via `tapa::invoke` from the host**; never
  called directly.
- An upper-level task body **must contain only** channel declarations and
  one `tapa::task().invoke(...)` chain. Scalar and mmap arguments may be
  forwarded to children, but the upper task performs no computation.
- Streams are passed **by reference** (`tapa::istream<T>&`,
  `tapa::ostream<T>&`), and so is `tapa::async_mmap<T>&`. Passing a stream or
  `async_mmap` by value is a compile error.
- mmap arguments are passed **by value** (`tapa::mmap<T>`), not by reference.
- Scalar arguments (plain C++ types such as `int`, `float`, `uint64_t`) are
  passed by value. A task may modify its local copy, but that modification is
  not returned to the host or parent; use an mmap or stream for results.
  Integer constant expressions passed through task `.invoke(...)`, including
  named `constexpr` and constant-initialized `const` integers, are converted to
  the child scalar parameter type, including numeric conversion to `float` and
  `double`.
  Floating-point constants, including named `constexpr` and `const` values, are
  rejected as invocation arguments.
  Runtime scalar bindings must refer to a parent-task parameter and use the same
  type (ignoring aliases and cv qualifiers) or equal-width non-boolean integer
  types. Other conversions are rejected; pass matching scalar types and convert
  inside a leaf task instead.
  A `const` runtime parameter is not a compile-time constant.
- Software simulation runs automatically when `tapa::invoke` receives an empty
  bitstream path.

---

## Common mistakes

### Wrong: calling the top-level task directly from host code

```cpp
// WRONG as a host entry point — bypasses backend selection, device buffer
// setup, and kernel-time reporting.
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

Which backend runs is decided by the bitstream path, as listed under
[Minimal correct example](#minimal-correct-example) above.

---

## See also

- [Tasks](tasks.md)
- [Streams](streams.md)
- [Memory Access: mmap](mmap.md)
- [Software Simulation](../howto/software-simulation.md)

**Next step:** [The Compile Pipeline](compile-pipeline.md)
