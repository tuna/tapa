# Lab 1: Vector Add

**Goal:** Understand why the VecAdd design is structured as four concurrent tasks connected by streams, and what each structural choice means for hardware generation.

**Prerequisites:** Complete [Your First Run](../start/first-run.md) so that you have already built and run the vadd example. This lab explains what you ran — it does not repeat the run commands.

**After this lab you will understand:**
- How a top-level task orchestrates leaf tasks without containing computation
- How mmap and stream arguments express data movement
- How the host invocation connects host memory to the hardware kernel

## Design overview

VecAdd computes `c[i] = a[i] + b[i]` for `n` elements. The implementation is a four-task pipeline:

```
Mmap2Stream(a) ──► a_q ──►
                           Add ──► c_q ──► Stream2Mmap(c)
Mmap2Stream(b) ──► b_q ──►
```

This is a producer-pipeline-consumer pattern. The two `Mmap2Stream` tasks read from global memory and feed elements into streams. `Add` consumes both streams and produces a result stream. `Stream2Mmap` drains the result stream back to global memory. All four tasks run concurrently once `VecAdd` is invoked — there is no sequencing between them.

The reason for this decomposition is not code style. TAPA generates separate hardware modules for each task, and the streams between them become FIFOs on the FPGA. When each stage is continuously supplied with data, the pipeline can run at full throughput.

## `Mmap2Stream`

```cpp
void Mmap2Stream(tapa::mmap<const float> mmap, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream << mmap[i];
  }
}
```

`tapa::mmap<const float>` is passed **by value**, not by reference. This is a hard rule in TAPA: mmap arguments to leaf tasks must be passed by value. The `const` qualifier marks the memory as read-only, which causes the compiler to generate a read-only AXI master port during synthesis. See [mmap](../concepts/mmap.md) for details.

Inside the loop, `mmap[i]` is array-style access to global memory. Each access becomes an AXI read transaction. The `<<` operator writes the element to the output stream, blocking if the FIFO is full. HLS can pipeline this loop at II=1 when the memory access latency is hidden by the pipeline depth.

## `Add`

```cpp
void Add(tapa::istream<float>& a, tapa::istream<float>& b,
         tapa::ostream<float>& c, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c << (a.read() + b.read());
  }
}
```

Stream arguments are passed **by reference**. This is the mirror of the mmap rule: streams must be by reference, mmap must be by value. See [Tasks](../concepts/tasks.md) for a full explanation.

`a.read()` blocks until an element is available in the FIFO. This is safe here because the loop runs exactly `n` times, and `Mmap2Stream` feeds exactly `n` elements into each stream. There is no risk of deadlock as long as the element counts match.

The `<<` on the output stream blocks if the downstream FIFO (`c_q`) is full. That backpressure propagates through the pipeline: `Add` stalls, which causes `a_q` and `b_q` to fill, which eventually stalls both `Mmap2Stream` tasks. The pipeline self-regulates without any explicit flow control logic.

HLS can pipeline this loop at II=1 because the operations (two reads and one add) are independent across iterations.

## `Stream2Mmap`

```cpp
void Stream2Mmap(tapa::istream<float>& stream, tapa::mmap<float> mmap,
                 uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    stream >> mmap[i];
  }
}
```

This is the mirror of `Mmap2Stream`. The `>>` operator reads one element from the stream (blocking) and writes it to global memory. The mmap is non-const this time because the output buffer is writable.

The same structural rules apply: mmap by value (non-const for write access), stream by reference.

## `VecAdd` — the top-level task

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

`VecAdd` contains **no computation** — no arithmetic, no memory access, no loops. This is deliberate. Upper-level tasks in TAPA are orchestration-only: they declare streams, then launch child tasks. Putting computation in an upper-level task is not supported.

The `tapa::stream<float>` declarations create named FIFOs. The string names (`"a"`, `"b"`, `"c"`) are used by TAPA's debug infrastructure: setting `TAPA_STREAM_LOG_DIR` causes TAPA to log every element transferred through each named stream, which is useful when tracking down data corruption.

The `.invoke()` chain starts all four child tasks simultaneously. TAPA does not sequence them — there is no "run Mmap2Stream first, then Add". All four tasks are live from the moment `VecAdd` is invoked, and they communicate entirely through the stream FIFOs. The task graph is what determines data ordering, not the order of `.invoke()` calls.

For a full description of the task graph model, see [The Programming Model](../concepts/programming-model.md).

```admonish note
The `.invoke()` chain is syntactic sugar for constructing a `tapa::task` object and calling `.invoke()` on it repeatedly. Each call returns the same task object, which is why chaining works. The task object goes out of scope at the end of `VecAdd`, which causes TAPA to wait for all child tasks to finish before returning.
```

## Host code

```cpp
int64_t kernel_time_ns = tapa::invoke(
    VecAdd, FLAGS_bitstream,
    tapa::read_only_mmap<const float>(a),
    tapa::read_only_mmap<const float>(b),
    tapa::write_only_mmap<float>(c), n);
```

`tapa::invoke` is the host-side entry point. It is not the same as calling `VecAdd()` directly: calling `VecAdd()` would run it as a plain C++ function (software simulation without timing), while `tapa::invoke` selects the execution mode based on the bitstream path:

- Empty string (`""`) — software simulation. TAPA runs `VecAdd` as C++ but with stream and mmap semantics enforced by the runtime library. Fast, no FPGA required.
- `.xo` file — fast cosimulation. The synthesized RTL runs inside a cycle-accurate simulator. Useful for verifying timing-sensitive behavior.
- `.xclbin` file — hardware execution on a real FPGA.

`tapa::read_only_mmap<const float>(a)` wraps the host vector `a` and tells the runtime to transfer it to the FPGA as a read-only buffer. `tapa::write_only_mmap<float>(c)` marks `c` as write-only, so the runtime transfers results back after the kernel finishes. These are directives to the runtime about transfer direction — they do not add C++ access restrictions beyond what the type already expresses.

For the actual build and run commands, see [Your First Run](../start/first-run.md).

## Rules summary

- **Leaf task arguments:** streams by reference (`tapa::istream<T>&`, `tapa::ostream<T>&`), mmap by value (`tapa::mmap<T>`)
- **Upper-level tasks:** declare streams with `tapa::stream<T>`, invoke child tasks with `.invoke()`, contain no computation
- **Stream names** (the string argument to `tapa::stream<T>`) are used by the debug infrastructure and appear in error messages — always name your streams
- **mmap const-ness** (`const float` vs `float`) determines whether the synthesized AXI master port is read-only or read-write; transfer direction at runtime is set separately by `read_only_mmap`/`write_only_mmap` on the host side

```admonish tip
If you see a compilation error about streams being passed by value or mmap being passed by reference, check your task signatures. TAPA enforces these argument-passing conventions at compile time.
```

**Next step:** [Lab 2: High-Bandwidth Memory](lab-02-async-mmap.md)
