# Lab 5: Parallel RTL Emulation

**Goal:** Compile kernel modules to RTL and simulate them concurrently while
software-modeled queues carry data between them.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and [Fast Hardware Simulation](../howto/fast-cosim.md).

After this lab you will understand how to use `tapa::executable` to assign
per-kernel RTL targets, compile each kernel to its own archive, run the
simulations in parallel, and prevent work-directory collisions.

The [Parallel RTL Emulation](../howto/parallel-rtl-emulation.md) guide covers
the concept, the accuracy limits of the shared-memory queues, the
`tapa::executable` API, and the runtime flags. This lab walks the same flow
through the Cannon matrix-multiply fixture in
`tests/functional/parallel-emulation/`.

---

## Step 1: Write the kernels

Each kernel is a plain TAPA task function. The Cannon example uses three
kernel functions — `Scatter`, `ProcElem`, and `Gather` — all in one source
file:

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

`beat_t`, declared in `cannon.h` as `tapa::vec_t<float, 16>`, is a 512-bit
memory beat, so each memory access moves 16 floats.

The top-level function declares the shared streams and gives each kernel its
own bitstream flag wrapped in `tapa::executable`:

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

When a `FLAGS_*_bitstream` flag is empty, that invocation falls back to
software simulation automatically, so you can bring up one kernel at a time.

---

## Step 2: Compile each kernel separately

Compile each function with `xilinx-hls` into its own ZIP archive and work
directory. The [compile
commands](../howto/parallel-rtl-emulation.md#compiling-each-kernel) are in
the guide, along with the reason the default `xilinx-vitis` target rejects
these leaf-top kernels.

---

## Step 3: Run parallel emulation

Build the host from `tests/functional/parallel-emulation/`, then pass all
three ZIP files to it. The cosim processes start concurrently:

```bash
tapa g++ -- cannon.cpp ../../apps/cannon/cannon-host.cpp -o cannon-host
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip
```

When you retain artifacts with `-cosim_work_dir`, add
`-cosim_work_dir_parallel` so the per-kernel processes do not collide — see
[Work directory](../howto/parallel-rtl-emulation.md#work-directory). The
guide's [concurrency
notes](../howto/parallel-rtl-emulation.md#controlling-host-side-concurrency)
explain why `TAPA_CONCURRENCY=1` simplifies scheduling without capping the
number of live simulators.

---

## Step 4: Verify

A successful run prints the application's correctness result (e.g., `PASS!`)
after all simulation processes finish. Diagnose failures the same way as
single-kernel cosim: add `-cosim_work_dir` and `-xsim_save_waveform` to
inspect per-kernel waveforms.
