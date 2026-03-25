# Lab 5: Parallel RTL Emulation

**Goal:** Compile cycle-sensitive kernel modules to RTL and simulate them concurrently, reducing total cosim time while preserving cycle-accurate behavior where it matters.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Fast Hardware Simulation](../howto/fast-cosim.md).

After this lab you will understand how to use `tapa::executable` to assign per-kernel RTL targets, compile each kernel to its own `.xo`, run the simulations in parallel, and prevent work-directory collisions between concurrent instances.

---

## When to use this

RTL cosimulation gives you cycle-accurate behavior for the logic inside each kernel — pipeline depths, stall conditions, hazards, and II violations that software simulation cannot catch. However, not everything needs this level of fidelity:

- **FIFOs between kernels** are modeled as shared-memory queues, not cycle-accurate RTL. The latency across kernel boundaries is not representative of hardware.
- **Memory accesses** (mmap, async_mmap) are similarly abstracted; memory latency is not cycle-accurate.

Parallel RTL emulation is therefore most valuable for validating the **cycle-sensitive internals** of each kernel in isolation — compute pipelines, II, resource usage — rather than end-to-end timing across the full datapath.

Running one cosim process per kernel and launching them concurrently reduces wall-clock time compared to simulating everything in a single process or sequentially. Use it when:

- Your design contains multiple kernels with non-trivial compute pipelines that need cycle-accurate validation.
- You want to catch pipeline hazards, incorrect II, or RTL-level bugs in each kernel before the expensive bitstream link step.
- The kernels can be compiled and simulated independently.

---

## Concept

In a standard single-kernel design, one top-level function compiles to one `.xo` and one cosim process validates it. In the parallel emulation pattern, several kernel functions compile independently and the host program runs one cosim process per kernel, all concurrently:

```
tapa::task()
  .invoke(KernelA, tapa::executable(FLAGS_a_bitstream), ...)  ──▶  cosim process A (cycle-accurate)
  .invoke(KernelB, tapa::executable(FLAGS_b_bitstream), ...)  ──▶  cosim process B (cycle-accurate)
  .invoke(KernelC, tapa::executable(FLAGS_c_bitstream), ...)  ──▶  cosim process C (cycle-accurate)
```

The streams connecting the processes are shared-memory FIFOs managed by the host runtime — latency-insensitive data transfer that lets each cosim process run at its own pace. Each kernel's internal cycle behavior is faithfully simulated; the inter-kernel communication is not.

---

## Step 1: Write the kernels

Each kernel is a plain TAPA task function. The Cannon matrix-multiply example from `tests/functional/parallel-emulation/` uses three kernel functions — `Scatter`, `ProcElem`, and `Gather` — all in one source file:

```cpp
// Distribute matrix rows into per-PE stream arrays
void Scatter(tapa::mmap<const float> matrix,
             tapa::ostreams<float, p * p>& block) { ... }

// Each PE computes its sub-matrix tile
void ProcElem(tapa::istream<float>& a_fifo, tapa::istream<float>& b_fifo,
              tapa::ostream<float>& c_fifo, ...) { ... }

// Collect PE results into the output matrix
void Gather(tapa::mmap<float> matrix,
            tapa::istreams<float, p * p>& block) { ... }
```

The top-level function declares the shared streams and assembles the task graph using `tapa::executable`:

```cpp
DEFINE_string(scatter_bitstream, "", "XO for Scatter; empty = software simulation");
DEFINE_string(proc_elem_bitstream, "", "XO for ProcElem; empty = software simulation");
DEFINE_string(gather_bitstream, "", "XO for Gather; empty = software simulation");

void Cannon(tapa::mmap<const float> a_vec, tapa::mmap<const float> b_vec,
            tapa::mmap<float> c_vec, uint64_t n) {
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

Each kernel function is compiled independently with its own `tapa compile --top` invocation:

```bash
tapa compile --top Scatter  --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o scatter.xo

tapa compile --top ProcElem --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o proc-elem.xo

tapa compile --top Gather   --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  -f cannon.cpp -o gather.xo
```

The three compilations read the same source file but each targets a different top function. The outputs are independent `.xo` files with no knowledge of each other.

---

## Step 3: Run parallel emulation

Pass all three `.xo` files to the host binary. All cosim processes start concurrently:

```bash
./cannon-host \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo
```

### Preventing work-directory collisions

By default each cosim process uses a temporary directory that is deleted at exit. When multiple processes share an explicit `-xosim_work_dir`, their intermediate files collide. Use `-xosim_work_dir_parallel_cosim` to give each process a unique subdirectory:

```bash
./cannon-host \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo \
    -xosim_work_dir ./cosim_work \
    -xosim_work_dir_parallel_cosim
```

TAPA creates `./cosim_work/XXXXXX/` per instance so the simulations do not interfere.

### Limiting concurrency

On memory-constrained machines, set `TAPA_CONCURRENCY` to cap the number of running cosim processes:

```bash
TAPA_CONCURRENCY=1 ./cannon-host \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo
```

Even with `TAPA_CONCURRENCY=1` the processes exchange data correctly through shared-memory FIFOs; they just run one at a time.

---

## Step 4: Verify

A successful run prints the application's correctness result (e.g., `PASS!`) after all simulation processes finish. Diagnose failures the same way as single-kernel cosim: add `-xosim_work_dir` and `-xosim_save_waveform` to inspect per-kernel waveforms.

---

## Further reading

[Parallel RTL Emulation](../howto/parallel-rtl-emulation.md) in the How-To Guides covers the full API reference, runtime flags, and additional invocation patterns.

---

**Next step:** [Lab 6: Floorplan & DSE](lab-06-floorplan.md)
