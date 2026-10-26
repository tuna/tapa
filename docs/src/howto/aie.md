# AI Engine Tasks

On a Versal device, a TAPA task can run on the AI Engine array instead of the
programmable logic (PL). The task keeps its C++ body; marking it moves it to an
AI Engine tile, and every stream between it and a PL task becomes a PLIO
connection. Software simulation is unchanged: an AI Engine task runs as an
ordinary task.

The working example is `tests/functional/aie-vadd/`.

## Marking a task

Mark a leaf task, or an upper task to put its whole subtree on the AI Engine:

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

`TASK:TARGET` takes `aie`, `hls` or `ignore`; an unknown task name is an
error. Inside an AI Engine upper task, a descendant explicitly marked for
another target is an error. The top task cannot be an AI Engine task: the PL
kernel owns the control interface. A hybrid top must also keep at least one
scalar or memory (`mmap`) argument: a top with only stream ports packages as
`ap_ctrl_none`, a free-running kernel XRT can neither start nor wait on, so
`tapa synth` rejects it. (A PL-only design may still be a stream top; it just
cannot be run through XRT.)

## What an AI Engine task may use

| Construct | On the AI Engine |
|---|---|
| `istream`, `ostream`, `istreams`, `ostreams` | Yes. Elements must be a multiple of 32 bits. |
| `read()`, `write()`, `>>`, `<<` | Yes, blocking. |
| `close`, `open`, `eot`, `try_eot`, `TAPA_WHILE_*EOT` | No. AI Engine streams carry no end-of-transmission token: loop over an element count. |
| `try_read`, `try_write`, `peek`, `empty`, `full` | No. AI Engine streams are blocking and expose no occupancy. |
| `mmap`, `async_mmap`, `mmaps`, `hmap` | No. Read memory in a PL task and stream the data in. |
| Scalars | Yes. |
| Local variables and arrays | Yes. They live on the kernel's stack, which the graph sizes as ADF's default 1 KB plus every local the body declares. The stack must fit in the tile's 32 KB data memory. |
| `[[tapa::pipeline]]`, `[[tapa::unroll]]` | Lowered to `chess_prepare_for_pipelining` and `chess_unroll_loop`. |

A PL task must not `close()` a stream that feeds an AI Engine task, because
the end-of-transmission token would be read as data.

An AI Engine task that a cycle of streams runs through, such as a ring of
tasks or a round trip through a PL task, keeps every stream access behind its
earlier writes, the order software simulation runs it in. The AI Engine
compiler would otherwise issue a read ahead of an earlier write, which the
cycle turns into a deadlock. The price is less overlap between the loop
iterations of that task; tasks off any cycle keep it. HLS keeps no such order
for a PL task, and within a pipelined loop may issue a read ahead of an
earlier write. A ring that also builds for PL puts its writes and its reads in
separate loops, writing no more at a time than its streams hold, as
`tests/apps/cannon` does.

These habits decide AI Engine performance:

- Write the inner loop with `tapa::vec_t`. On the AI Engine a native `vec_t<float, 4>`
  is one vector register, `a + b` is one vector add, and `acc += a * b` is one
  multiply-accumulate. Other whole-word `vec_t` shapes use scalar operations;
  scalar loops run far slower than native vector code.
- Use a 32-bit loop counter and `[[tapa::pipeline]]` on the hot loop. A 64-bit
  counter keeps the loop from being software-pipelined.
- Accumulate into a local `vec_t` variable, not an array element. The AI
  Engine compiler keeps an array in memory, so each multiply-accumulate would
  wait on a load and a store.
- Move whole vectors rather than single lanes. Reading or writing one lane of
  a `vec_t` costs several instructions and a branch, even in an unrolled
  loop, and in the hot loop it costs more than the arithmetic it feeds.
  `tests/apps/cannon` keeps the rows of B as vectors and sends them around its
  ring whole.

## Knobs

| Attribute | Where | Effect |
|---|---|---|
| `[[tapa::aie_frame(N)]]` | `istream`/`ostream` parameter of an AI Engine leaf | The stream arrives as frames of `N` elements in tile memory, filled by DMA, instead of through the core's stream port. The body is unchanged. A final partial output frame is zero-padded; an input frame releases and discards unused padding when the task returns. |
| `[[tapa::aie_plio(W)]]` | `istream`/`ostream` parameter of an AI Engine task | PLIO width `W` of 32, 64 or 128 bits for a boundary stream. |
| `[[tapa::aie_location(col, row)]]` | AI Engine leaf task | Pins the task to a tile. Add a third argument to pick one instance of a task invoked several times. |

A task built for PL ignores these attributes, so a source annotated for the AI
Engine still builds for PL when `--target` leaves the task there.

## Simulating the AI Engine tasks

Software simulation runs AI Engine tasks as ordinary tasks. To check the AI
Engine build of those tasks, record the streams a software simulation moves
and replay the ones that cross into the AI Engine:

```bash
mkdir logs
TAPA_STREAM_LOG_DIR=logs ./vadd-host 256
tapa --work-dir work.out analyze -f vadd.cpp -t VecAdd
tapa --work-dir work.out aiesim --stimulus logs --arg n=256 \
  --part-num xcvc1902-vsva2197-2MP-e-S
```

`tapa aiesim` generates the ADF graph under `work.out/aie/sim/`, compiles it
for `x86simulator`, feeds each PL-to-AI Engine stream from its
`<stream>.words` log (zero-padded to a whole frame and PLIO beat), and compares
every AI Engine-to-PL stream with its log. Only zero padding is ignored;
surplus or nonzero output is an error.
A boundary stream is found by its variable name, so name the stream after its
variable: `tapa::stream<vec4f> a_q("a_q")`. Pass every top-task scalar the AI
Engine tasks read with `--arg`.

`--cycle-accurate` builds for hardware and runs `aiesimulator` instead, and
prints when the first and last beat of each output stream arrived and the mean
spacing between beats:

```text
c_q: 1024 of 1024 words, beats at 230.4..1862.4 ns (6.40 ns apart)
AI Engine simulation matches software simulation
```

When the work directory holds the `report.json` / `report.yaml` a previous
`tapa synth` wrote, a run in which every output stream matches also folds
that timing into their `aie` block under `aiesim`, preserving the rest of
the report:

```yaml
aie:
  aiesim:
    streams:
      c_q:
        beats: 256
        first_beat_ns: 230.4
        last_beat_ns: 1862.4
        mean_beat_spacing_ns: 6.4
```

A stream with a single beat reports no `mean_beat_spacing_ns`, since it has
no interval to average. `tapa aiesim` also works on a work directory that
has only been analyzed, which has no report yet; then no report is created
and the timing stays on the console. Reports from before a later `analyze`
or a failed synth are stale and are not updated. A run with any output
mismatch leaves both files untouched.

On an AI Engine 1 device, a kernel reads about 32 bits from its input streams
per AI Engine cycle in total, however many stream ports it reads. That is one
128-bit beat every 3.2 ns at 1.25 GHz, so the two-input add above runs at
6.4 ns per output beat. Measure each kernel this way rather than planning on
the port count. A frame port (`[[tapa::aie_frame(N)]]`) is filled by DMA and
does not consume that budget.

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
the top task's body, binds each of those streams by name, and runs in
software the invocations on the AI Engine side of them. The PL invocations run
in the kernel, as in any `.xo` cosimulation. As with `tapa aiesim`, name each
boundary stream after its variable.

## Building and linking

`tapa compile` (or `analyze`, `synth`, `pack`) builds both halves. The flow
must be `xilinx-vitis`, because `v++` links the AI Engine graph with the PL
kernel:

```bash
tapa --work-dir work.out compile -f vadd.cpp -t VecAdd \
  --platform xilinx_vck190_base_202520_1 -o vadd.xo -s link.sh
```

- `tapa synth` compiles the AI Engine tasks into `work.out/aie/hw/libadf.a`
  with `aiecompiler --target=hw`, which needs the AI Engine compiler licence,
  and synthesizes only the PL tasks. Each PL-to-AI Engine stream becomes an
  AXIS port of the PL kernel named after the stream, and its PLIO runs at the
  PL kernel clock. It warns about each such stream whose elements are
  narrower than 128 bits: the PL moves one element per cycle, and only a
  128-bit PLIO keeps up with an AI Engine stream port.

  After a successful hardware AI Engine build, `report.json` and `report.yaml`
  have a root-level `aie` block. It records `tiles_used`, a `plio_map` with each
  logical PLIO's direction, interface column, width and consumed 64-bit
  channels, and `per_kernel` tile and program-memory usage (`data_bytes` is
  omitted when unknown). PL-only builds and `--skip-aie` omit this block. For
  the supported AIE1 flow, synth also logs each occupied interface column's
  usage against 8 PL-to-AI Engine and 6 AI Engine-to-PL 64-bit channels; a
  128-bit PLIO consumes two channels. A mapping over either limit is an error.
- `tapa pack` writes the PL kernel as `vadd.xo` and, next to it, the graph as
  `vadd.libadf.a` and the PLIO connectivity as `vadd.aie.cfg`, one
  `stream_connect` per boundary stream between the kernel's compute unit
  (named after the top task) and `ai_engine_0`.
- The `-s` script links the three into an `.xsa` and packages that into the
  `.xclbin`.

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
mapping. The rewrite removes `aie.aiesim`: run `tapa aiesim --cycle-accurate`
again to measure the new graph. Without valid synthesized PL state, AIE-only
does not invent a PL report or refresh stale reports; it also does not create
root reports when none exist. The compiler output and console remain available
until a later full PL synth writes the root report.

A stream into the AI Engine asserts TLAST on every beat. The AI Engine
interface would otherwise hold a lone 32-bit word until a second one arrives.

## Running the xclbin

`tapa::invoke` on the `.xclbin` runs both halves through XRT, with the
unchanged host program. Before it starts the PL kernel, the runtime writes
each scalar argument an AI Engine task reads into that task's runtime
parameter and runs the graph for one iteration. Once the PL kernel is done,
it waits for the graph to finish. A graph with a detached task runs until the
PL kernel is done and is then stopped; the next `tapa::invoke` resets it, so
its tasks start again from the top. The reported kernel time is still the PL
kernel's, from start to done.

This control path has not yet run on an AI Engine device.

## VCK190 system emulation

A VCK190 `hw_emu` run includes the PL RTL, the AI Engine simulator, and an
aarch64 Linux guest running the unchanged host through XRT. It requires Vitis
with the VCK190 base platform and the matching extracted
`xilinx-versal-common` image; no physical board is used. Set `VERSAL_COMMON`
to the directory containing `Image` and `rootfs.ext4`, then run:

```bash
bazel test --config=vck190 --test_env=VERSAL_COMMON \
  //tests/functional/aie-vadd:aie-vadd-hw-emu
```

The `--config=vck190` platform cross-builds the guest host while TAPA itself
continues to run on the build machine. The test compiles the design, creates an
SD image, launches QEMU and the hardware simulators, and invokes the guest host
twice. It is a manual, vendor-gated test and can take hours. Set
`AIE_HW_EMU_TIMEOUT` to override its default 7200-second emulator timeout.

For another design, pass the common-image files and any guest applications to
`tapa pack` (or `tapa compile`) together with `-s`:

```bash
tapa compile -f vadd.cpp -t VecAdd --platform xilinx_vck190_base_202520_1 \
  -o vadd.xo -s link.sh \
  --package-kernel-image "$VERSAL_COMMON/Image" \
  --package-rootfs "$VERSAL_COMMON/rootfs.ext4" \
  --package-sd-file ./vadd-host-aarch64
TARGET=hw_emu ./link.sh
./launch_hw_emu.sh -run-app './vadd-host-aarch64 --bitstream=VecAdd_xilinx_vck190_base_202520_1.xclbin 64'
```

The package options require an AI Engine graph and a bitstream script. The
script sets SD boot mode and defers automatic AI Engine startup so FRT can
write runtime parameters before starting the graph. Without these options,
existing PCIe and PL-only scripts are unchanged.
