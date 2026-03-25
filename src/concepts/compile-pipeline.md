# The Compile Pipeline

**Purpose:** Understand the three-stage TAPA compile pipeline.

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
    ▼  tapa analyze  (always local)
task graph JSON
    │
    ▼  tapa synth    (can run remotely, parallelizable with -j)
per-task RTL (Verilog)
    │
    ▼  tapa pack     (can run remotely)
.xo file
    ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ (TAPA boundary) ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
    │
    ▼  v++ --link    (Vitis, not TAPA)
.xclbin
```

**`tapa analyze`** — Runs `tapa-cpp` and `tapacc` locally. Reads your C++
source, resolves task boundaries, and writes a task graph JSON to the work
directory. No vendor tools are required for this step.

**`tapa synth`** — Invokes Vitis HLS for each leaf task to produce per-task
Verilog RTL. This is the most time-consuming step. With `-j N`, up to N tasks
are synthesized in parallel. With `--remote-host`, synthesis runs on a remote
Linux machine that has Vitis HLS installed.

**`tapa pack`** — Combines the per-task RTL and the top-level wrapper generated
by `tapa analyze` into a single Xilinx IP package (`.xo` file) suitable for
`v++ --link`.

**Shortcut:** `tapa compile` runs all three stages in the correct order in a
single command.

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

Run `tapa analyze` first to extract the task graph (no vendor tools needed):

```bash
tapa analyze \
  --top VecAdd \
  -f vadd.cpp \
  --work-dir work.out
```

Then run `tapa synth` to synthesize each task to RTL, optionally in parallel
and/or on a remote host:

```bash
tapa synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  --work-dir work.out \
  -j 4
```

Finally, run `tapa pack` to produce the `.xo` file:

```bash
tapa pack \
  --work-dir work.out \
  -o vecadd.xo
```

---

## Rules

- `tapa analyze` **always runs locally**, even when `--remote-host` is set.
- `tapa synth` and `tapa pack` run on the remote host when `--remote-host` is
  provided.
- `tapa compile` is the shortcut for all three stages and handles stage
  ordering automatically.
- The `-j` / `--jobs` flag on `tapa synth` controls how many Vitis HLS
  processes run in parallel. Keep it at or below the available core count on
  the synthesis machine.
- The work directory (`--work-dir`, default `work.out`) must be the same across
  all three stages when running them separately.

---

## Common mistakes

### Wrong: running `tapa synth` before `tapa analyze`

```bash
# WRONG — the task graph JSON does not exist yet; tapa synth will fail
# with a missing file error.
tapa synth --part-num xcu250-figd2104-2L-e --clock-period 3.33 --work-dir work.out
```

### Right: always run `tapa analyze` first, or use `tapa compile`

```bash
# RIGHT — explicit ordering
tapa analyze --top VecAdd -f vadd.cpp --work-dir work.out
tapa synth   --part-num xcu250-figd2104-2L-e --clock-period 3.33 --work-dir work.out
tapa pack    --work-dir work.out -o vecadd.xo

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
