# Parallel RTL Emulation

**Purpose:** Simulate separately compiled kernel RTL concurrently while using
software-modeled queues between the kernels.

Each kernel's own RTL is simulated cycle by cycle, which can expose pipeline
and handshake bugs that software simulation misses. The complete system is
not cycle-accurate: inter-kernel FIFOs are shared-memory queues, and external
memory latency is modeled functionally. Use this flow for per-kernel RTL
validation and inter-kernel dataflow, not end-to-end timing measurement.

Running independent simulators concurrently can reduce wall-clock time
compared with running them sequentially, at the cost of additional CPU and
memory use.

---

## Concept

In a standard TAPA design, one top-level function is compiled into one `.xo` and the entire design is simulated as a single cosim process. In the parallel emulation pattern:

- Each kernel function is compiled to its own `.xo` or `.zip` with
  `tapa compile --top <KernelFunc>`.
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
// Kernel-specific override: run KernelFunc from the given RTL archive.
task& invoke(Func&& func, tapa::executable exe, Args&&... args);
```

All `.invoke()` calls in a `tapa::task()` chain start concurrently. Kernels that receive a `tapa::executable` each get their own cosim process; kernels without one run as software coroutines.

```admonish note
Pass `tapa::executable` immediately after the kernel function, before the
kernel's other arguments.
```

---

## Compiling Each Kernel

Each kernel function is compiled independently. Invoke `tapa compile` once per top function, passing its name via `--top`:

```bash
tapa --work-dir work.out/scatter compile \
  --target xilinx-hls \
  --top Scatter \
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o scatter.zip

tapa --work-dir work.out/proc-elem compile \
  --target xilinx-hls \
  --top ProcElem \
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o proc-elem.zip

tapa --work-dir work.out/gather compile \
  --target xilinx-hls \
  --top Gather \
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f cannon.cpp \
  -o gather.zip
```

All three functions are leaf tasks, so the default `xilinx-vitis` target
would reject them as top-level kernels. Use `xilinx-hls`, which permits a leaf
top and produces ZIP archives. Separate work directories keep the three
compiler states independent. Each archive contains only its own top
function's interface.

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

`TopFunction` runs in software simulation and owns the stream objects. The
runtime connects those streams to shared-memory FIFOs used by the cosim
processes. Data and backpressure still drive execution, but inter-kernel queue
timing is not representative of hardware.

---

## Running

Pass the compiled RTL archives to the host binary:

```bash
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip
```

When any flag is empty the corresponding kernel runs in software simulation. This lets you emulate a subset of the design while the rest runs in simulation:

```bash
# Only emulate ProcElem; Scatter and Gather run in software simulation.
./cannon-host --proc_elem_bitstream=proc-elem.zip
```

### Work directory

By default each cosim process writes to a temporary directory that is deleted at exit. Provide `-cosim_work_dir` to retain artifacts. When multiple kernels share the same work directory their simulation environments collide; use `-cosim_work_dir_parallel` to give each process a unique subdirectory:

```bash
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip \
    -cosim_work_dir ./cosim_work \
    -cosim_work_dir_parallel
```

TAPA creates `./cosim_work/<pid>_<nanoseconds>/` (a unique name per instance) so the simulations do not interfere with each other.

### Simulator backend

The same `-cosim_simulator` flag applies to all instances:

```bash
./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip \
    -cosim_simulator verilator
```

### Controlling host-side concurrency

`TAPA_CONCURRENCY` sets how many worker threads the host runtime uses to run
its task coroutines (default: the host's physical core count). Lowering it
reduces host thread pressure and simplifies the scheduling interleaving:

```bash
TAPA_CONCURRENCY=1 ./cannon-host \
    --scatter_bitstream=scatter.zip \
    --proc_elem_bitstream=proc-elem.zip \
    --gather_bitstream=gather.zip
```

```admonish warning
`TAPA_CONCURRENCY` does **not** cap how many simulators run at once. Each invocation that receives a `tapa::executable` spawns its own simulator process and the host coroutine then yields while it runs, so every simulator is live concurrently even at `TAPA_CONCURRENCY=1`. If you are running out of memory, emulate fewer kernels per run — leave the other bitstream flags empty so those kernels fall back to software simulation.
```

---

## Runtime flags reference

The shared cosim flags are documented in [Runtime
Flags](../reference/runtime-flags.md#host-executable-flags-fast-cosim). The
rows below are specific to running multiple concurrent simulations:

| Flag | Description |
|------|-------------|
| `-cosim_work_dir_parallel` | Create a unique subdirectory per instance. Required when multiple kernels share `-cosim_work_dir`. |
| `TAPA_CONCURRENCY` | Environment variable. Number of host worker threads running task coroutines — not a cap on simulator processes. |

---

## Complete example

The `tests/functional/parallel-emulation/` directory contains the Cannon
matrix-multiply fixture used for this pattern. `Scatter` and `Gather` handle
memory, while a grid of `ProcElem` instances computes and exchanges tiles.
From that directory, build the host with:

```bash
tapa g++ -- cannon.cpp ../../apps/cannon/cannon-host.cpp -o cannon-host
```

Then compile the three `xilinx-hls` ZIP archives as shown above and pass them
to `./cannon-host`. Its `BUILD.bazel` file also provides ready-made software
and cosimulation test targets.

---

**See also:** [Fast Hardware Simulation](fast-cosim.md) — single-kernel cosim with the same `-cosim_*` and `-xsim_*` flags.
