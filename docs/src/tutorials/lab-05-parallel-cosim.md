# Lab 5: Parallel RTL Emulation

**Goal:** Compile kernel modules to RTL and simulate them concurrently while
software-modeled queues carry data between them.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Fast Hardware Simulation](../howto/fast-cosim.md).

After this lab you will understand how to use `tapa::executable` to assign
per-kernel RTL targets, compile each kernel to its own archive, run the
simulations in parallel, and prevent work-directory collisions.

---

## When to use this

Each kernel's own RTL is simulated cycle by cycle, which can expose pipeline
and handshake bugs that software simulation misses. Two important parts of
the system remain abstract:

- **FIFOs between kernels** are shared-memory queues, so inter-kernel latency
  is not representative of hardware.
- **Memory accesses** (mmap and async_mmap) use functional models rather than
  cycle-accurate memory timing.

Use parallel RTL emulation for per-kernel RTL validation and inter-kernel
dataflow, not end-to-end timing or resource measurement.

Running one cosim process per kernel concurrently can reduce wall-clock time
compared with running them sequentially, at the cost of additional CPU and
memory use. Use it when:

- Your design contains multiple kernels with non-trivial compute pipelines that need RTL validation.
- You want to catch pipeline hazards, incorrect II, or RTL-level bugs in each kernel before the expensive bitstream link step.
- The kernels can be compiled and simulated independently.

---

## Concept

In a standard single-kernel design, one top-level function compiles to one `.xo` and one cosim process validates it. In the parallel emulation pattern, several kernel functions compile independently and the host program runs one cosim process per kernel, all concurrently:

```
tapa::task()
  .invoke(KernelA, tapa::executable(FLAGS_a_bitstream), ...)  ──▶  kernel RTL process A
  .invoke(KernelB, tapa::executable(FLAGS_b_bitstream), ...)  ──▶  kernel RTL process B
  .invoke(KernelC, tapa::executable(FLAGS_c_bitstream), ...)  ──▶  kernel RTL process C
```

Shared-memory FIFOs managed by the host runtime connect the processes and
let each simulator run at its own pace. They preserve dataflow and
backpressure, but not inter-kernel timing.

---

## Step 1: Write the kernels

Each kernel is a plain TAPA task function. The Cannon matrix-multiply example from `tests/functional/parallel-emulation/` uses three kernel functions — `Scatter`, `ProcElem`, and `Gather` — all in one source file:

```cpp
// Distribute matrix rows into per-PE stream arrays
void Scatter(tapa::mmap<const beat_t> matrix,
             tapa::ostreams<float, p * p>& block) { ... }

// Each PE computes its sub-matrix tile
void ProcElem(tapa::istream<float>& a_fifo, tapa::istream<float>& b_fifo,
              tapa::ostream<float>& c_fifo, ...) { ... }

// Collect PE results into the output matrix
void Gather(tapa::mmap<beat_t> matrix,
            tapa::istreams<float, p * p>& block) { ... }
```

`beat_t`, declared in `cannon.h`, is a 512-bit memory beat of 16 floats, so
each memory access moves 16 floats.

The top-level function declares the shared streams and assembles the task graph using `tapa::executable`:

```cpp
DEFINE_string(scatter_bitstream, "", "RTL archive for Scatter; empty = software simulation");
DEFINE_string(proc_elem_bitstream, "", "RTL archive for ProcElem; empty = software simulation");
DEFINE_string(gather_bitstream, "", "RTL archive for Gather; empty = software simulation");

void Cannon(tapa::mmap<const beat_t> a_vec, tapa::mmap<const beat_t> b_vec,
            tapa::mmap<beat_t> c_vec, uint64_t n) {
  tapa::streams<float, p * p> a("a"), b("b"), c("c");
  // ... inter-PE streams ...

  tapa::task()
      .invoke(Scatter, tapa::executable(FLAGS_scatter_bitstream), a_vec, a)
      .invoke(Scatter, tapa::executable(FLAGS_scatter_bitstream), b_vec, b)
      .invoke(ProcElem, tapa::executable(FLAGS_proc_elem_bitstream), a, b, c, ...)
      // ... more ProcElem instances ...
      .invoke(Gather, tapa::executable(FLAGS_gather_bitstream), c_vec, c);
}
```

When a `FLAGS_*_bitstream` flag is empty, that invocation falls back to software simulation automatically. This lets you bring up one kernel at a time.

---

## Step 2: Compile each kernel separately

Each function in this example is a leaf task. The default `xilinx-vitis`
target rejects a leaf top, so compile with `xilinx-hls` and produce a separate
ZIP archive and work directory for each kernel:

```bash
tapa --work-dir work.out/scatter compile \
  --target xilinx-hls --top Scatter \
  --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o scatter.zip

tapa --work-dir work.out/proc-elem compile \
  --target xilinx-hls --top ProcElem \
  --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o proc-elem.zip

tapa --work-dir work.out/gather compile \
  --target xilinx-hls --top Gather \
  --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o gather.zip
```

The three compilations read the same source file but select different top
functions. The independent ZIP archives contain only their own kernel
interfaces. Do not use the default Vitis target for these leaf-top kernels.

---

## Step 3: Run parallel emulation

Build the host from `tests/functional/parallel-emulation/`, then pass all
three ZIP files to it. The cosim processes start concurrently:

```bash
tapa g++ -- cannon.cpp ../../apps/cannon/cannon-host.cpp -o cannon-host
```

```bash
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip
```

### Preventing work-directory collisions

By default each cosim process uses a temporary directory that is deleted at exit. When multiple processes share an explicit `-cosim_work_dir`, their intermediate files collide. Use `-cosim_work_dir_parallel` to give each process a unique subdirectory:

```bash
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip \
    -cosim_work_dir ./cosim_work \
    -cosim_work_dir_parallel
```

TAPA creates `./cosim_work/XXXXXX/` per instance so the simulations do not interfere.

### Controlling host-side concurrency

`TAPA_CONCURRENCY` sets the number of worker threads the host runtime uses
for its task coroutines (default: the host's physical core count). Setting it
to 1 simplifies the scheduling interleaving, which can help reproduce a
failure:

```bash
TAPA_CONCURRENCY=1 ./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip
```

```admonish warning
This is a host-side thread count, not a simulator budget. Each `tapa::executable` invocation spawns its own simulator process and the host coroutine yields while it runs, so all simulators are live at once even at `TAPA_CONCURRENCY=1`. To cut memory use, emulate fewer kernels per run by leaving the other bitstream flags empty.
```

---

## Step 4: Verify

A successful run prints the application's correctness result (e.g., `PASS!`) after all simulation processes finish. Diagnose failures the same way as single-kernel cosim: add `-cosim_work_dir` and `-xsim_save_waveform` to inspect per-kernel waveforms.

---

## Further reading

[Parallel RTL Emulation](../howto/parallel-rtl-emulation.md) in the How-To Guides covers the full API reference, runtime flags, and additional invocation patterns.
