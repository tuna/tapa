# Parallel RTL Emulation

**Purpose:** Run cycle-accurate RTL simulation for each kernel module concurrently, reducing total cosim time while preserving cycle-accurate behavior where it matters.

RTL cosimulation gives you cycle-accurate behavior for the logic *inside* each kernel — pipeline depths, stall conditions, II violations, and hazards that software simulation cannot catch. It does not give you cycle-accurate behavior *between* kernels: the FIFOs connecting separate cosim processes are shared-memory queues, and memory (mmap/async_mmap) latency is similarly abstracted. Parallel RTL emulation is therefore most valuable for validating the **cycle-sensitive internals** of individual kernels, not end-to-end timing across the full datapath.

Running one cosim process per kernel and launching them concurrently reduces wall-clock time compared to simulating everything in a single process or sequentially.

---

## Concept

In a standard TAPA design, one top-level function is compiled into one `.xo` and the entire design is simulated as a single cosim process. In the parallel emulation pattern:

- Each kernel function is compiled to its own `.xo` with `tapa compile --top <KernelFunc>`.
- The host application defines a separate bitstream flag per kernel and passes each to `.invoke()` wrapped in `tapa::executable`.
- `tapa::task` launches all kernel simulations concurrently; streams between kernels communicate through shared memory files managed by the runtime.

```
┌────────────────────────────────────────────────────────┐
│  Host application                                      │
│                                                        │
│  tapa::task()                                          │
│    .invoke(KernelA, tapa::executable(FLAGS_a_bs), ...) │──▶ cosim process A
│    .invoke(KernelB, tapa::executable(FLAGS_b_bs), ...) │──▶ cosim process B
│    .invoke(KernelC, tapa::executable(FLAGS_c_bs), ...) │──▶ cosim process C
└────────────────────────────────────────────────────────┘
         streams between kernels → shared-memory FIFOs (not cycle-accurate)
```

---

## API

### `tapa::executable`

Wraps a path to a kernel `.xo` (or `.zip` for the `xilinx-hls` target). When passed as the second argument to `.invoke()`, the runtime launches RTL emulation for that invocation instead of running it in software simulation.

```cpp
class executable {
 public:
  explicit executable(std::string path);
  // Not copyable or movable.
};
```

If the path is empty, `.invoke()` falls back to software simulation for that kernel. This lets a single binary select simulation or emulation per-kernel at runtime.

### `tapa::task::invoke` with `tapa::executable`

```cpp
// Kernel-specific override: run KernelFunc from the given XO file.
task& invoke(Func&& func, tapa::executable exe, Args&&... args);
```

All `.invoke()` calls in a `tapa::task()` chain start concurrently. Kernels that receive a `tapa::executable` each get their own cosim process; kernels without one run as software coroutines.

```admonish note
`tapa::executable` must be provided before any argument that is a direct stream reader or writer.  The runtime uses the executable path to bind the right simulation backend before it can connect streams.
```

---

## Compiling Each Kernel

Each kernel function is compiled independently. Invoke `tapa compile` once per top function, passing its name via `--top`:

```bash
tapa compile \
  --top Scatter \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o scatter.xo

tapa compile \
  --top ProcElem \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o proc-elem.xo

tapa compile \
  --top Gather \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o gather.xo
```

All three compilations can share the same source file. Each produces an independent `.xo` that knows only its own top function's interface.

---

## Host Code

The host application follows the standard TAPA pattern, but uses one `DEFINE_string` per kernel rather than a single `--bitstream` flag:

```cpp
#include <gflags/gflags.h>
#include <tapa.h>

DEFINE_string(scatter_bitstream, "",
              "path to Scatter XO; empty = software simulation");
DEFINE_string(proc_elem_bitstream, "",
              "path to ProcElem XO; empty = software simulation");
DEFINE_string(gather_bitstream, "",
              "path to Gather XO; empty = software simulation");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);
  // ... allocate buffers ...

  tapa::invoke(TopFunction, /*bitstream=*/"",
               tapa::read_only_mmap<const float>(a),
               tapa::read_only_mmap<const float>(b),
               tapa::write_only_mmap<float>(c), n);
}
```

The `TopFunction` assembles the task graph. Each `.invoke()` receives its own `tapa::executable`:

```cpp
void TopFunction(tapa::mmap<const float> a_vec,
                 tapa::mmap<const float> b_vec,
                 tapa::mmap<float> c_vec, uint64_t n) {
  tapa::streams<float, 4> a("a");
  tapa::streams<float, 4> b("b");
  tapa::streams<float, 4> c("c");
  // ... declare inter-kernel streams ...

  tapa::task()
      .invoke(Scatter, tapa::executable(FLAGS_scatter_bitstream), a_vec, a)
      .invoke(Scatter, tapa::executable(FLAGS_scatter_bitstream), b_vec, b)
      .invoke(ProcElem, tapa::executable(FLAGS_proc_elem_bitstream), a, b, c, ...)
      // ... more ProcElem instances ...
      .invoke(Gather, tapa::executable(FLAGS_gather_bitstream), c_vec, c);
}
```

Streams declared inside `TopFunction` are host-side objects. The runtime passes references to the same shared-memory FIFO to each cosim process that reads or writes it, so data flows between kernels exactly as it would on hardware.

---

## Running

Pass the compiled `.xo` files to the host binary:

```bash
./cannon \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo
```

When any flag is empty the corresponding kernel runs in software simulation. This lets you emulate a subset of the design while the rest runs in simulation:

```bash
# Only emulate ProcElem; Scatter and Gather run in software simulation.
./cannon --proc_elem_bitstream=proc-elem.xo
```

### Work directory

By default each cosim process writes to a temporary directory that is deleted at exit. Provide `-cosim_work_dir` to retain artifacts. When multiple kernels share the same work directory their simulation environments collide; use `-cosim_work_dir_parallel` to give each process a unique subdirectory:

```bash
./cannon \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo \
    -cosim_work_dir ./cosim_work \
    -cosim_work_dir_parallel
```

TAPA creates `./cosim_work/XXXXXX/` (a unique name per instance) so the simulations do not interfere with each other.

### Simulator backend

The same `-cosim_simulator` flag applies to all instances:

```bash
./cannon \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo \
    -cosim_simulator verilator
```

### Controlling concurrency

Set `TAPA_CONCURRENCY` to limit how many cosim processes run simultaneously. This is useful on machines with limited memory:

```bash
TAPA_CONCURRENCY=1 ./cannon \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo
```

At `TAPA_CONCURRENCY=1` the processes still exchange data correctly through shared-memory FIFOs, but only one simulation runs at a time.

---

## Runtime flags reference

| Flag | Description |
|------|-------------|
| `-cosim_work_dir <dir>` | Persistent working directory for simulation artifacts. |
| `-cosim_work_dir_parallel` | Create a unique subdirectory per instance. Required when multiple kernels share `-cosim_work_dir`. |
| `-cosim_simulator <backend>` | `xsim` (default, Linux only) or `verilator` (cross-platform). Applied to all instances. |
| `-xsim_save_waveform` | Save simulation waveforms. Pair with `-cosim_work_dir`. |
| `-cosim_executable <path>` | Deprecated. Fast cosim now runs in-process via `libfrt`; this flag is ignored. |
| `-xsim_part_num <part>` | Target FPGA part number (e.g., `xcu280-fsvh2892-2L-e`). |
| `TAPA_CONCURRENCY` | Environment variable. Limits the number of cosim processes that run simultaneously. |

---

## Full example: Cannon matrix multiply

The `tests/functional/parallel-emulation/` directory in the TAPA repository contains a working parallel-emulation example. The Cannon algorithm splits into three kernels:

| Kernel | Role |
|--------|------|
| `Scatter` (×2) | Distributes rows of matrices A and B into per-PE stream arrays |
| `ProcElem` (×p²) | Each PE computes its sub-matrix tile and shifts blocks to neighbours |
| `Gather` (×1) | Collects results from all PEs into the output matrix |

**Compile** (three invocations from one source file):

```bash
tapa compile --top Scatter  -f cannon.cpp -o scatter.xo   --part-num xcu280-fsvh2892-2L-e --clock-period 3.33
tapa compile --top ProcElem -f cannon.cpp -o proc-elem.xo --part-num xcu280-fsvh2892-2L-e --clock-period 3.33
tapa compile --top Gather   -f cannon.cpp -o gather.xo    --part-num xcu280-fsvh2892-2L-e --clock-period 3.33
```

**Run:**

```bash
./cannon-host \
    --scatter_bitstream=scatter.xo \
    --proc_elem_bitstream=proc-elem.xo \
    --gather_bitstream=gather.xo \
    -cosim_work_dir ./cosim_work \
    -cosim_work_dir_parallel
```

A successful run prints `PASS!` after all simulation processes finish.

---

**See also:** [Fast Hardware Simulation](fast-cosim.md) — single-kernel cosim with the same `-cosim_*` and `-xsim_*` flags.
