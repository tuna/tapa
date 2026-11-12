# AI Engine Tasks

On a Versal device, a TAPA task can run on the AI Engine array instead of the
programmable logic (PL). The task keeps its C++ body; marking it moves it to an
AI Engine tile, and every stream between it and a PL task becomes a PLIO
connection. Software simulation is unchanged: an AI Engine task runs as an
ordinary task.

The working example is `tests/functional/aie-vadd/`.
`tests/functional/aie-roundtrip/` exercises two AIE stages separated by a PL
stage, covering PL → AIE → PL → AIE → PL data movement.

## Marking a task

Mark a leaf task, or an upper task to make the AI Engine the default for the
tasks below it:

```cpp
using vec4f = tapa::vec_t<float, 4>;

[[tapa::target("aie")]]
void Add(tapa::istream<vec4f>& a, tapa::istream<vec4f>& b,
         tapa::ostream<vec4f>& c, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) c.write(a.read() + b.read());
}
```

The same choice can be made on the command line without editing the source,
so one source builds both a PL-only and a PL+AI Engine design. The mapping wins
over the attribute:

```bash
tapa analyze ... --target xilinx-vitis --target Add:aie
```

`TASK:TARGET` takes `aie`, `hls` or `ignore`; a name that is no task under the
top, such as a typo or a helper function, is an error. An upper task's target
is only a default: a task below it with a target of its own, from the
attribute or the command line, keeps that target and hands it down instead. An
AI Engine upper task may therefore invoke `[[tapa::target("hls")]]` tasks, and
a PL upper task may invoke AI Engine tasks. A task that would inherit the AI
Engine through one invoker but is also invoked by a PL task is an error; give
it a target. Marking the top task makes the AI Engine the default for the
tasks it invokes, while the top itself stays the PL kernel, which owns the
control interface, unless it needs none (see [A top with no PL
kernel](#a-top-with-no-pl-kernel)); a leaf top cannot be marked.

Every stream between a PL task and an AI Engine task crosses into the array
through a PLIO channel, and `tapa analyze` lists each crossing. An upper task
whose tasks run on both is dissolved into its parent when the design is
built: its streams become the parent's, named `<instance>_<stream>` (such as
`Stage_0_mid`), so it must be invoked joined or detached and must invoke its
own tasks joined or detached. Hybrid cosimulation and a simulator package
run the body of such a task on the host and split the run among the tasks
it invokes, so a crossing may be declared at any level.

A hybrid top must also keep at least one scalar or memory (`mmap`) argument,
or a stream argument carried by a ring ([Frames from and to the
host](../concepts/streams.md#frames-from-and-to-the-host)), which XRT sees
as a memory argument: a top with only AXIS stream ports packages as
`ap_ctrl_none`, a free-running kernel XRT can neither start nor wait on, so
`tapa synth` rejects it. (A PL-only design may still be such a stream top;
it just cannot be run through XRT.)

## What an AI Engine task may use

| Construct | On the AI Engine |
|---|---|
| `istream`, `ostream`, `istreams`, `ostreams` | Yes. Elements must be a multiple of 32 bits, and at most 128 bits on a stream to or from the PL. |
| `iframes`, `oframes` | Yes: the kernel's buffer ports, filled or drained by DMA; `acquire()` takes the next buffer and its view addresses tile memory directly. Between two AI Engine frame ports the graph connects the buffers, so neighbouring kernels share tile memory and others are joined by DMA. A PL task feeds or drains one through an ordinary `ostream` or `istream`, or through a frame port of its own, whose frame channel sits in the PL kernel's top. See [frame ports](../concepts/streams.md#frame-ports). |
| `read()`, `write()`, `>>`, `<<`, `read(dst, n)`, `write(src, n)` | Yes, blocking. |
| `close`, `open`, `eot`, `try_eot`, `TAPA_WHILE_*EOT` | No. AI Engine streams carry no end-of-transmission token: loop over an element count. |
| `try_read`, `try_write` | Accepted, but they block on the AI Engine. Use them only where the schedule guarantees progress. |
| `peek`, `empty`, `full` | No. AI Engine streams expose no occupancy. |
| `async_mmap` | Yes, served from the PL; see [Memory](#memory). Elements must be 32, 64 or 128 bits. |
| `mmap`, `mmaps`, `hmap` | No, except a `tapa::mmap` an upper task hands on (see [Memory](#memory)). Use an `async_mmap`, or read memory in a PL task and stream the data in. |
| Scalars | Yes. One bound to a scalar argument of the top is a runtime parameter, and must be an 8-, 16-, 32- or 64-bit integer, a `bool` or a `float`; an invoke-site constant is compiled into the kernel. |
| Local variables and arrays | Yes. They live on the kernel's stack, which the graph sizes as ADF's default 1 KB plus every local the body and the functions it calls declare. The stack must fit in the tile's 32 KB data memory. |
| `[[tapa::pipeline]]` on a loop | Lowered to `chess_prepare_for_pipelining`. Every loop nested in it is fully unrolled, as HLS does when it pipelines a loop; `[[tapa::unroll(k)]]` on a nested loop unrolls it `k` times instead. On a function it is ignored with a remark: put it on the loop. |
| `[[tapa::unroll]]` | Lowered to `chess_flatten_loop`, the AI Engine compiler's full unroll, when full and `chess_unroll_loop(k)` with a factor. |
| `tapa::wait()`, `tapa::reg()` | Accepted. As in software simulation, `wait` does nothing and `reg` returns its argument: the core has no PL clock cycle to yield. |
| `[[tapa::tripcount(min, max)]]` on a loop | Lowered to `chess_loop_range(min, max)`. HLS only reports the range, but the AI Engine compiler optimizes on it: a loop that runs fewer than `min` or more than `max` times is undefined behavior on the AI Engine. |
| HLS-only attributes such as `partition`, `dependence` and `flatten` | Ignored with a remark; they still affect the PL build. The AI Engine compiler does not merge loop nests, so `flatten` has nothing to control there. |

A PL task must not `close()` a stream that feeds an AI Engine task, because
the end-of-transmission token would be read as data; `tapa analyze` rejects
it.

An AI Engine task that a cycle of streams runs through, such as a ring of
tasks or a round trip through a PL task, keeps every stream access behind its
earlier writes, the order software simulation runs it in. The AI Engine
compiler would otherwise issue a read ahead of an earlier write, which the
cycle turns into a deadlock. The price is less overlap between the loop
iterations of that task; tasks off any cycle keep it. HLS keeps no such order
for a PL task: it may issue a read in the same cycle as an earlier write, or
ahead of it.

`tapa::fence()` keeps every stream access after it behind every one before
it: on the AI Engine as a scheduler barrier, in HLS as a clock boundary, and
in software simulation, which runs in program order, as a no-op. A ring that
also builds for PL fences its writes from its reads and writes no more
between its reads than its streams hold. A source whose tasks
place one between each loop that writes a cycle and the loop that reads it
may define `TAPA_AIE_PHASE_ORDERED_STREAMS` before including `tapa.h`; those
tasks then skip the fence after every write, which is what lets a loop that
computes also write the cycle at full speed.

These habits decide AI Engine performance:

- Write the inner loop with `tapa::vec_t`. On the AI Engine a native `vec_t<float, 4>`
  is one vector register, `a + b` is one vector add, and `acc += a * b` is one
  multiply-accumulate. Other whole-word `vec_t` shapes use scalar operations;
  scalar loops run far slower than native vector code.
- Use a 32-bit loop counter and `[[tapa::pipeline]]` on the hot loop. A 64-bit
  counter keeps the loop from being software-pipelined. Pipeline the loops
  that fill and drain local arrays too, so that a short loop nested in them,
  such as one over the parts of a vector, is unrolled as well.
- Accumulate into a local `vec_t` variable, not an array element. The AI
  Engine compiler keeps an array in memory, so each multiply-accumulate would
  wait on a load and a store.
- Move whole vectors rather than single lanes. Reading or writing one lane of
  a `vec_t` costs several instructions and a branch, even in an unrolled
  loop, and in the hot loop it costs more than the arithmetic it feeds.
- A lane that only feeds a multiply is the exception: `acc += x[k] * b`
  broadcasts lane `k` of a non-const native `x` inside the
  multiply-accumulate, with no extract. Copy `x` out of its array into a
  local first, so it is loaded once for all its lanes. `tests/apps/cannon`
  keeps A, B and C as rows of vectors and multiplies each row of B by one lane
  of A this way.
- Give each chain of multiply-accumulates its own accumulator. A float
  multiply-accumulate into the result of the one before it issues every
  second cycle; independent accumulators fill the cycles in between.
  `tests/apps/cannon` computes four vectors of a row of C per iteration, one
  accumulator each.
- Move a frame port's data whole, with `read(dst, n)` and `write(src, n)`.
  They test for a frame boundary once per run of a frame, where `read()` and
  `write()` test twice per element, and the DMA moves the frames while the
  core computes; the core's own stream ports move only 32 bits per cycle.
  `tests/apps/cannon` carries its ring in frames this way: each round but the
  last, a PE writes its blocks of A and B whole, multiplies, and then reads
  its neighbors' blocks whole. Its blocks from and to the PL travel as two
  halves on two frame ports, so over two PLIO channels at once.

## Memory

An AI Engine task reaches memory through a `tapa::async_mmap<T>&` port bound
to a memory argument of the top. The AI Engine has no memory interface of its
own: for each direction the body uses, TAPA adds a PL task to the PL kernel,
bound to that memory, and joins it to the task over PLIO. The same source
runs as a PL task too. An AI Engine upper task in between hands on either
its `async_mmap<T>&`, to one task, or a `tapa::mmap<T>`, which each of its
tasks takes as its own `async_mmap` with its own PL tasks.

```cpp
[[tapa::target("aie")]] void Scale(tapa::async_mmap<float>& src,
                                   tapa::async_mmap<float>& dst, uint64_t n) {
  float tile[kTile];
  src.read_addr.write({0, kTile});  // request tile 0
  for (uint64_t t = 0; t < n / kTile; ++t) {
    if (t + 1 < n / kTile) src.read_addr.write({(t + 1) * kTile, kTile});
    src.read_data.read(tile, kTile);  // tile t, while t + 1 is on its way
    for (int i = 0; i < kTile; ++i) tile[i] *= 2;
    dst.write_addr.write({t * kTile, kTile});
    dst.write_data.write(tile, kTile);
  }
}
```

- Request a range at once with a burst, `{offset, n}` (see
  [async_mmap](../concepts/async-mmap.md)). Requesting the next tile before
  computing this one hides the round trip through the PL and memory behind
  the computation.
- Every operation blocks, as on AI Engine streams: request ahead instead of
  polling. `full()` is always false.
- A write request's data follows it before the next write request: the two
  share one stream port, since a kernel has only two output stream ports.
- Reading `write_resp` is optional. The PL task waits for every write to be
  acknowledged before the PL kernel finishes, so a body that only needs its
  writes done by then can leave the acknowledgements alone. A body that
  reads them reads every one before it returns, as on PL.
- Reads cost two PLIO channels (requests, data) and the kernel an output and
  an input stream port; writes cost one channel and an output port, plus an
  input port and a channel when the body reads `write_resp`. Together with
  its stream ports they must fit the kernel's two inputs and two outputs.
- No PL task may bind the same memory: nothing orders its accesses against
  the AI Engine task's.
- Software simulation serves the port from host memory, as for a PL task.
  The AI Engine simulators and hybrid cosimulation serve it too (see below),
  wherever the task is invoked.

## A top with no PL kernel

A top marked `[[tapa::target("aie")]]` stays on the AI Engine when no task
below it runs on the PL and it takes no memory (memory needs the PL tasks
above). The design then has no PL kernel: the top's scalars are the graph's
runtime parameters, and its stream arguments move frames between host
memory and the array over GMIO, one transfer per frame.

```cpp
[[tapa::target("aie")]]
void Pipeline(tapa::istream<int>& src, tapa::ostream<int>& dst, int n) {
  tapa::stream<int, 2 * kFrame> mid("mid");
  tapa::task().invoke(AddOne, src, mid, n).invoke(Scale, mid, dst, n);
}
```

- Each stream argument must reach a frame port of a task below:
  `tapa::iframes` / `tapa::oframes`, or a `[[tapa::frame(N)]]` stream port.
  A transfer is one frame, so a frame port addresses it directly in tile
  memory.
- The tasks writing the output arguments report how many frames they
  released, and a run ends once those frames have reached the host. Invoke
  every task joined: a detached one never reports, and is rejected.
- The host program is unchanged: it passes the top streams as usual. On
  XRT each stream argument keeps one transfer in flight, so a frame should
  be large enough to cover the round trip to the host. Each output's
  transfer for a frame that might come next is retired after the run by a
  drain run: the graph runs once more, every kernel skipping its task and
  releasing one frame on each output it writes, which the runtime discards.
  Input frames the graph did not read stay for the next invoke, so push
  only what it reads.
- GMIO needs XRT on the board's own processor, as on the embedded VCK190.
  A VCK5000 reaches its array only through PL, so there TAPA builds a PL
  kernel named after the top instead ([below](#on-a-vck5000)).

With nothing to make an `.xo` of, `tapa pack` writes a `.zip` holding the
graph and the top's argument list. `tapa link` links the graph alone,
records the argument list in the xclbin, and leaves the `.xsa` beside it;
`--sim` links for hardware emulation. The SD image the board or QEMU boots
comes from `v++ --package` with the Versal common image and the host
program, which loads that xclbin:

```bash
tapa --work-dir work.out compile -f pipeline.cpp -t Pipeline \
  --platform xilinx_vck190_base_202520_1 -o pipeline.zip
tapa --work-dir work.out link --package pipeline.zip --sim \
  -o pipeline.hw_emu.xclbin
unzip pipeline.zip libadf.a
v++ --package --target hw_emu --platform xilinx_vck190_base_202520_1 \
  --package.boot_mode sd --package.defer_aie_run \
  --package.kernel_image $COMMON/Image --package.rootfs $COMMON/rootfs.ext4 \
  --package.sd_file pipeline-host --package.sd_file pipeline.hw_emu.xclbin \
  --package.out_dir package -o package/sd.xclbin pipeline.hw_emu.xsa libadf.a
cd package && ./launch_hw_emu.sh \
  -run-app "./pipeline-host --bitstream=pipeline.hw_emu.xclbin"
```

Build the host program for the board's processor; in this repository,
`--config=vck190` cross-builds it, and
`//tests/functional/aie-gmio:aie-gmio-hw-emu` runs the steps above
(`--test_env=VERSAL_COMMON=<common image>`).

### On a VCK5000

The same source builds for a VCK5000, whose graph TAPA's PL controller
drives. The top's PL kernel is TAPA's: each stream argument is a ring of
frames in device memory, as for a PL top ([Frames from and to the
host](../concepts/streams.md#frames-from-and-to-the-host)), which moves its
frames to or from the graph over a PLIO named `<argument>_plio`. The task
writing an output argument sends how many frames it released, once it
returns, on a 32-bit PLIO `<argument>_count`; the kernel is done once that
many frames are in memory. Its scalars stay runtime parameters, which the
PL controller writes. `[[tapa::ring(S)]]` on a stream argument sets the
ring's frames, unused where GMIO moves them.

`tapa compile` writes an `.xo` with the graph and the PL controller beside
it, which `tapa link` links like a hybrid design's. Each output argument
takes two AI Engine-to-PL PLIOs, its frames and its count: two of the six
64-bit channels an interface column has in that direction, or three when its
elements are wider than 64 bits; the PL controller takes one more. With
`--skip-aie` the kernel runs in hybrid cosimulation, the AI Engine tasks in
software ([Hybrid cosimulation](#hybrid-cosimulation));
`//tests/functional/aie-gmio:aie-gmio-vck5000-hybrid` runs it.

## Knobs

| Knob | Where | Effect |
|---|---|---|
| `[[tapa::frame(N)]]` | `istream`/`ostream` parameter of a leaf task | The port moves whole frames of `N` elements, and its stream must hold at least one: `tapa analyze` rejects a shallower stream on every target, since the AI Engine fills a frame before its reader sees any of it. On the AI Engine the frames arrive in tile memory, filled by DMA, instead of through the core's stream port. The body is unchanged. A final partial output frame is zero-padded; an input frame releases and discards unused padding when the task returns. On PL the port is an ordinary stream, and on Vitis a stream argument of the top that reaches it moves through a [ring](../concepts/streams.md#frames-from-and-to-the-host). Where both ends of a stream move frames, here or through `tapa::iframes` / `tapa::oframes`, they must agree on `N`. |
| `tapa analyze --aie-plio TASK.PORT:W` | `istream`, `ostream`, `iframes` or `oframes` port of an AI Engine task | PLIO width `W` of 32, 64 or 128 bits for a boundary stream; the default is the element width, rounded up to one of these. |
| `tapa analyze --aie-location TASK:COL,ROW[,INSTANCE]` | AI Engine leaf task | Pins the task to a tile; `INSTANCE` picks one instance of a task invoked several times. |

Placement and PLIO widths depend on the device, so they stay out of the
source; a source annotated for the AI Engine still builds for PL when
`--target` leaves the task there.

Where the two targets want different code, guard the AI Engine side with
`#if defined(TAPA_TARGET_AIE_)`. The AI Engine compile defines it, on x86sim
as on hardware; software simulation, tapacc and HLS compile the other branch.
tapacc therefore neither checks nor lowers the guarded code, so it should use
only ports and locals the other branch also uses, and a TAPA attribute such
as `[[tapa::pipeline]]` takes no effect there: tapacc removes it, with a
remark, as it does in any code the preprocessor skips. `tests/apps/cannon` guards where its
PEs send their blocks: whole before multiplying on the AI Engine, a chunk at a
time while multiplying on PL.

## Simulating the AI Engine tasks

Software simulation runs AI Engine tasks as ordinary tasks. To check their AI
Engine build, compile the graph as a simulator package and run the unchanged
host program against it:

```bash
tapa --work-dir work.out compile -f vadd.cpp -t VecAdd \
  --part-num xcvc1902-vsva2197-2MP-e-S --aie-sim x86sim -o vadd.x86sim
./vadd-host --bitstream=vadd.x86sim 256
```

`--aie-sim x86sim` builds the ADF graph for `x86simulator` into the package
directory and nothing else: there is no HLS run. Given the package, the host
program runs every PL task in software, starts the simulator, and relays each
PL↔AI Engine stream between them as the data flows, so a stream cycle through
the PL runs as on hardware. A top with no PL kernel has only the host's
streams to relay: the graph's entry point moves each frame over GMIO as it
arrives. The scalars the AI Engine tasks read come from
the host's arguments. Software simulation remains the run without a
bitstream. Running a package needs the simulators on `PATH`: source the
Vitis `settings64.sh`.

`--part-num` reaches `aiecompiler` as `--part`, which older releases lack:
Vitis 2022.2, the only one that targets the VCK5000, rejects it. There, name
the platform instead (`--platform xilinx_vck5000_gen4x8_qdma_2_202220_1`),
whose `.xpfm` carries the part; a platform installed on the machine running
`tapa` is used in preference to `--part-num` whenever both are given.

The host binds each PL↔AI Engine stream by name, so name it after its
variable: `tapa::stream<vec4f> a_q("a_q")`. An `async_mmap` port needs no
name: the host runs its PL movers in software, on the memory the host
program passed, against the simulator. A stream into the AI Engine ends
when the PL task writing it returns; it is then zero-padded to a whole frame
and PLIO beat. A final partial output frame arrives zero-padded, as on
hardware, and its padding is left unread in the stream.

`--aie-sim aiesim` builds the graph for the cycle-approximate `aiesimulator`
instead, with `aiecompiler --target=hw` as for hardware, which needs the AI
Engine compiler licence. It reads each input in full when it starts, so it
cannot be fed as the data flows: the host program runs the whole design in
software, keeping a copy of each PL↔AI Engine stream, and then the
simulator replays the copies of the inputs. For a top with no PL kernel the
copies are of its stream arguments: the frames software simulation read
from each input and wrote to each output. The graph's entry point posts
every input frame before the run and room for as many output frames as
software simulation wrote; the kernels must report that many. A replay runs
from `tapa::invoke`; the host fills the input streams before it and drains
the outputs after it, so they must hold a whole run. An `async_mmap` port's
channels are copied too: the requests and writes the kernel makes must
match those of software simulation, which hands it the data that software
simulation read. Each output it writes must match what software simulation
wrote to that stream, padding aside, or the run fails at the first differing
word. Each output is printed with the simulated time of its beats:

```text
dst_q: 256 of 256 words, beats at 256.0..656.0 ns (6.35 ns apart)
AI Engine simulation matches software simulation
```

The times are the AI Engine's alone, fed as fast as its inputs allow; the PL
tasks ran in software, and nothing the PL does with an output reaches the
simulator.

On an AI Engine 1 device, a kernel reads about 32 bits from its input streams
per AI Engine cycle in total, however many stream ports it reads. That is one
128-bit beat every 3.2 ns at 1.25 GHz, so the two-input add above runs at
6.4 ns per output beat. Measure each kernel with `--aie-sim aiesim` rather
than planning on the port count. A frame port (`[[tapa::frame(N)]]`) is
filled by DMA and does not consume that budget.

## Hybrid cosimulation

The PL half of the design can run as RTL while the AI Engine tasks run in
software, driven by the unchanged host program. Build the PL kernel without
the AI Engine graph and pass the `.xo` as the bitstream:

```bash
tapa --work-dir work.out compile -f vadd.cpp -t VecAdd \
  --platform xilinx_vck190_base_202520_1 --skip-aie -o vadd.xo
./vadd-host --bitstream=vadd.xo 256
```

The kernel leaves one AXIS stream per AI Engine stream to the host. TAPA runs
the top task's body, and the body of every upper task dissolved into it,
binds each of those streams by name, and runs in software the invocations on
the AI Engine side of them. The PL invocations run in the kernel, as in any
`.xo` cosimulation. As with a simulator package, name each boundary stream
after its variable. An AI Engine task's `async_mmap` runs in software too,
its requests carried over the kernel's streams to the PL movers in the RTL.

The PL kernel of a top with no PL task on a VCK5000 ([On a
VCK5000](#on-a-vck5000)) runs the same way: the host's streams feed its
rings, and the top's tasks run in software on streams of their own, bound
to the kernel's PLIOs; each output's frame count goes to the kernel when
the task writing it returns. Elsewhere such a top has no PL half, and
`tapa synth` rejects `--skip-aie` for it.

## Building and linking

`tapa compile` (or `analyze`, `synth`, `pack`) builds both halves. The flow
must be `xilinx-vitis`, because `v++` links the AI Engine graph with the PL
kernel:

```bash
tapa --work-dir work.out compile -f vadd.cpp -t VecAdd \
  --platform xilinx_vck190_base_202520_1 -o vadd.xo
tapa link --package vadd.xo -o vadd.xclbin
```

- `tapa synth` compiles the AI Engine tasks into `work.out/aie/hw/libadf.a`
  with `aiecompiler --target=hw`, which needs the AI Engine compiler licence,
  and synthesizes only the PL tasks. Each PL↔AI Engine stream becomes an
  AXIS port of the PL kernel named after the stream, and its PLIO runs at the
  PL kernel clock. It warns about each such stream whose elements are
  narrower than 128 bits: the PL moves one element per cycle, and only a
  128-bit PLIO keeps up with an AI Engine stream port.

  After a successful hardware AI Engine build, `report.json` and `report.yaml`
  have a root-level `aie` block. It records `tiles_used`, a `plio_map` with each
  logical PLIO's direction, interface column, width and consumed 64-bit
  channels, and `per_kernel` tile and program-memory usage. PL-only builds
  and `--skip-aie` omit this block. For
  the supported AIE1 flow, synth also logs each occupied interface column's
  usage against 8 PL-to-AI Engine and 6 AI Engine-to-PL 64-bit channels; a
  128-bit PLIO consumes two channels. A mapping over either limit is an error.
- `tapa pack` writes the PL kernel as `vadd.xo` and, next to it, the graph as
  `vadd.libadf.a` and the PLIO connectivity as `vadd.aie.cfg`, one
  `stream_connect` per boundary stream between the kernel's compute unit
  (named after the top task) and `ai_engine_0`. A VCK5000 build (on a
  platform whose name contains `vck5000`, such as
  `xilinx_vck5000_gen4x8_qdma_2_202220_1`) also writes
  `vadd.aie_control.xo`, a PL kernel that controls the graph through two
  32-bit PLIOs of its own, `tapa_ctrl` into the graph and `tapa_ctrl_ret`
  back, one 64-bit channel in each direction; no APU control program is
  needed.
- `tapa link` finds the sidecars next to the `.xo`, links the graph, PL XOs and
  connectivity into an `.xsa`, and packages that into the `.xclbin`.

A stream into the AI Engine asserts TLAST on every beat, and the last word of
every element an AI Engine task writes toward the PL carries TLAST, which the
PL ignores. The AI Engine interface would otherwise hold a lone 32-bit word,
in either direction, until a second one arrives.

### Iterating on the AI Engine only

After `tapa analyze`, rebuild just the hardware graph with the usual device and
clock arguments:

```bash
tapa --work-dir work.out synth --aie-only \
  --platform xilinx_vck190_base_202520_1
```

`--aie-only` is the counterpart of `--skip-aie`, and the two flags cannot be
combined. It requires an AI Engine partition and does not stage or run HLS,
generate RTL, run post-synthesis utilization, or print a PL timing summary. A
successful build replaces `work.out/aie/hw/libadf.a` and makes a later pack
expect its sidecar; a failed build does not turn a full build into an
intentional PL-only package.

Existing PL results and floorplan state remain reusable only if the resolved
part number, platform, and requested clock period are identical to those of
the previous PL synthesis. Repeating AIE-only with those unchanged inputs
preserves `flow.synthed=true` and the floorplan. Changing any input clears both
the synthesized flag and floorplan, so run a full `tapa synth` before packing.
A work directory invalidated by `tapa analyze` is never promoted to synthesized
by AIE-only. In every case the typed PL metrics already in `tapa.json` are
preserved rather than replaced with zero or missing measurements.

If valid synthesized PL state and current `report.json` / `report.yaml` files
exist, AIE-only refreshes them from those preserved metrics and the new hardware
mapping. Without valid synthesized PL state, AIE-only
does not invent a PL report or refresh stale reports; it also does not create
root reports when none exist. The compiler output and console remain available
until a later full PL synth writes the root report.

### Mixing Vitis releases

Each tool runs from its own installation, whose `settings64.sh` TAPA sources
first: `vitis_hls` from `XILINX_HLS` and `vivado` from `XILINX_VIVADO`, each
falling back to `XILINX_VITIS`, and `aiecompiler` and `v++` from
`XILINX_VITIS`. Running the steps one at a time, each with its own roots,
compiles with a newer release than the platform's. The VCK5000 platform
`xilinx_vck5000_gen4x8_qdma_2_202220_1` is a Vitis 2022.2 platform, whose AI
Engine compiler miscompiles `tests/apps/fft2d` and stalls on
`tests/apps/cannon`, and PL frame ports need Vitis HLS 2024.1 or newer; so
synthesize with 2024.1, then pack and link with 2022.2:

```bash
export XILINX_VITIS=/opt/Xilinx/Vitis/2024.1 \
  XILINX_HLS=/opt/Xilinx/Vitis_HLS/2024.1 XILINX_VIVADO=/opt/Xilinx/Vivado/2024.1
tapa --work-dir work.out analyze -f fft2d.cpp -t Fft2d \
  --target=RowPass:aie --target=ColPass:aie
tapa --work-dir work.out synth --platform xilinx_vck5000_gen4x8_qdma_2_202220_1
export XILINX_VITIS=/opt/Xilinx/Vitis/2022.2 \
  XILINX_HLS=/opt/Xilinx/Vitis_HLS/2022.2 XILINX_VIVADO=/opt/Xilinx/Vivado/2022.2
tapa --work-dir work.out pack -o fft2d.xo
tapa link --package fft2d.xo -o fft2d.xclbin
```

Vivado 2022.2 and 2023.1 hang in `opt_design` on the RTL that Vitis HLS
2024.1 writes for the PL frame ports of `tests/functional/frame-views`;
synthesize those with Vitis HLS 2025.2 instead.

## Running the xclbin

`tapa::invoke` on the `.xclbin` runs both halves through XRT, with the
unchanged host program. Before it starts the PL kernel, the runtime writes
each scalar argument an AI Engine task reads into that task's runtime
parameter and runs the graph for one iteration. Once the PL kernel is done,
it waits for the graph to finish. A graph with a detached task runs until the
PL kernel is done and is then stopped; the next `tapa::invoke` resets it, so
its tasks start again from the top. The reported kernel time is still the PL
kernel's, from start to done.

VCK190 uses XRT's native graph API. On VCK5000, FRT instead sends register
writes to `tapa_aie_pl_control` in the PL: runtime parameters, the iteration
count and core-enable before the PL kernel starts, then core-disable after it
completes. The graph does not run before that: `tapa link` packages it with its
start deferred to the controller. No APU control program or host `xrtGraphOpen`
call is used. Each write reaches only the tile it names: a packet split in the
graph gives each kernel's packet controller its own packet id, which `tapa
pack` compiles into the controller from the compiled graph's reports. Each
kernel's stack, and the iteration count beside it, stays on the kernel's tile;
FRT requires single-buffered RTPs and rejects a runtime parameter or iteration
count outside the kernels' tiles.

On VCK5000, the next `tapa::invoke` restarts a graph with a detached task
through the controller too, before the writes above: it resets the cores,
then the DMA channels and locks of the buffers behind its frame ports, so
each task starts again at its first frame whatever number of frames the
last invoke moved. Such a graph keeps the buffers of a frame port to the PL
in its kernel's tile. A frame port joined to another AI Engine task's shares
a buffer the compiler places, and `tapa pack` rejects the graph if that
buffer lies outside the kernels' tiles. An AI Engine tile performs a lock
operation when it is read, so the controller reads each lock and takes the
answers on `tapa_ctrl_ret`.
