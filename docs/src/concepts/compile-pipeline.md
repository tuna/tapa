# The Compile Pipeline

**Purpose:** Understand the stages of the TAPA compile pipeline.

**Prerequisites:** [The Programming Model](programming-model.md)

Each `tapa` subcommand maps to one pipeline stage. Knowing the stages helps
diagnose failures, parallelize synthesis, and use remote execution correctly.

---

## Why this exists

Compiling a TAPA design involves three distinct concerns: parsing C++ and
extracting the task graph, synthesizing each task to RTL with Vitis HLS, and
packaging the RTL into an `.xo` file for Vitis. Separating these stages lets
you re-run only the parts that changed, run synthesis on a remote machine with
Xilinx tools, and parallelize synthesis across tasks.

---

## Mental model

```
C++ source
    │
    ▼  tapa analyze   (always local)
task graph JSON
    │
    ▼  tapa synth     (can run remotely, parallelizable with -j)
per-task RTL (Verilog)
    │
    ▼  tapa floorplan (optional; multi-SLR devices only)
placement + timing constraints
    │
    ▼  tapa pack      (can run remotely)
.xo file
    ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ (TAPA boundary) ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
    │
    ▼  v++ --link     (Vitis, not TAPA)
.xclbin
```

**`tapa analyze`** — Runs `tapa-cpp` and `tapacc` locally. Reads your C++
source, resolves task boundaries, and writes the task graph to `tapa.json` in
the work directory. No vendor tools are required for this step.

**`tapa synth`** — Invokes Vitis HLS for each task to produce per-task
Verilog RTL. This is the most time-consuming step. With `-j N`, up to N tasks
are synthesized in parallel. With `--remote-host`, synthesis runs on a remote
Linux machine that has Vitis HLS installed.

**`tapa floorplan`** *(optional)* — Assigns tasks to SLRs on a multi-die
device, balances resource usage across them, and inserts pipeline registers on
the channels that cross SLR boundaries. It writes `floorplan.xdc`, which
`pack` picks up automatically. Skip it if your design already meets timing;
see [Performance Tuning](../howto/performance-tuning.md) for when it helps.

**`tapa pack`** — Combines the per-task RTL into a single Xilinx IP package
(`.xo` file) suitable for `v++ --link`.

**Shortcut:** `tapa compile` runs analyze, synth, and pack in the correct
order in a single command. It does not floorplan — for that, run the steps
separately with `tapa floorplan` between `synth` and `pack`.

---

## Minimal correct example

### All-in-one (most common)

```bash
tapa compile \
  --top VecAdd \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vecadd.xo
```

Use `--platform` instead of `--part-num` when targeting a full Vitis platform:

```bash
tapa compile \
  --top VecAdd \
  --platform xilinx_u250_gen3x16_xdma_4_1_202210_1 \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vecadd.xo
```

### Running stages separately

`--work-dir` is a top-level `tapa` flag that applies to all subcommands. It
must be the same across all three stages when running them separately (default:
`work.out`).

Run `tapa analyze` first to extract the task graph (no vendor tools needed):

```bash
tapa --work-dir work.out analyze \
  --top VecAdd \
  -f vadd.cpp
```

Then run `tapa synth` to synthesize each task to RTL, optionally in parallel
and/or on a remote host:

```bash
tapa --work-dir work.out synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -j 4
```

Finally, run `tapa pack` to produce the `.xo` file:

```bash
tapa --work-dir work.out pack \
  -o vecadd.xo
```

---

## Rules

- `tapa analyze` **always runs locally**, even when `--remote-host` is set.
- `tapa synth` and `tapa pack` run on the remote host when `--remote-host` is
  provided, as does `tapa floorplan` when it is measuring an implementation
  (`--run-impl` / `--dse`).
- `tapa compile` is the shortcut for analyze, synth, and pack, and handles
  stage ordering automatically. Floorplanning requires running the steps
  separately.
- The `-j` / `--jobs` flag on `tapa synth` controls how many Vitis HLS
  processes run in parallel. Keep it at or below the available core count on
  the synthesis machine.
- `--work-dir` is a top-level flag: `tapa --work-dir DIR <subcommand>`.

---

## Common mistakes

### Wrong: running `tapa synth` before `tapa analyze`

```bash
# WRONG — the task graph JSON does not exist yet; tapa synth will fail
# with a missing file error.
tapa --work-dir work.out synth --part-num xcu250-figd2104-2L-e --clock-period 3.33
```

### Right: always run `tapa analyze` first, or use `tapa compile`

```bash
# RIGHT — explicit ordering
tapa --work-dir work.out analyze --top VecAdd -f vadd.cpp
tapa --work-dir work.out synth   --part-num xcu250-figd2104-2L-e --clock-period 3.33
tapa --work-dir work.out pack    -o vecadd.xo

# RIGHT — shortcut that handles ordering automatically
tapa compile --top VecAdd --part-num xcu250-figd2104-2L-e \
             --clock-period 3.33 -f vadd.cpp -o vecadd.xo
```

---

## Note about v++ link

```admonish note
The `v++ --link` step that produces `.xclbin` is performed by Xilinx Vitis,
not TAPA. TAPA's output is the `.xo` file. See
[Build & Run on Board](../howto/build-and-run.md) for the full linking
workflow.
```

---

## See also

- [Build & Run on Board](../howto/build-and-run.md)
- [Remote Execution](../howto/remote-execution.md)
- [CLI Commands](../reference/cli.md)

**Next step:** [Tasks](tasks.md)
