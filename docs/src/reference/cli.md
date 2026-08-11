# CLI Commands

Reference for all `tapa` CLI subcommands. For task-oriented guides, see [Build and Run](../howto/build-and-run.md) and the other How-To pages. The general invocation form is:

```
tapa [global options] <subcommand> [subcommand options]
```

```admonish note
`tapa compile` is a shortcut that runs `tapa analyze`, `tapa synth`, and `tapa pack` in sequence in a single command. When using the individual subcommands, pass `--work-dir` as a **global** flag before the subcommand name: `tapa --work-dir DIR <subcommand>`.
```

Subcommands are also chainable — they are processed left to right, sharing one work directory, so `tapa analyze … synth … pack …` is a single invocation:

```bash
tapa --work-dir work.out \
  analyze --top VecAdd -f vadd.cpp \
  synth --part-num xcu250-figd2104-2L-e --clock-period 3.33 \
  pack -o vadd.xo
```

This page covers the flags you need for day-to-day work. A few additional flags exist for build-system plumbing and internal testing; run `tapa <subcommand> --help` to see the complete surface.

## Global Options

These options must appear before the subcommand name.

| Flag | Description |
|------|-------------|
| `--work-dir DIR` / `-w DIR` | Working directory for intermediate artifacts (default: `./work.out/`). |
| `--temp-dir DIR` | Temporary directory, exported to child tools through `TMPDIR`. |
| `--verbose` / `-v` | Increase logging verbosity. Repeatable (e.g., `-vv`). |
| `--quiet` / `-q` | Decrease logging verbosity. |
| `--clang-format-quota-in-bytes N` | Only run `clang-format` over the first `N` bytes of generated code (default: `1000000`). Lower it to speed up runs on very large designs. |
| `--remote-host user@host[:port]` | Remote Linux host where vendor tools run. |
| `--remote-key-file PATH` | SSH private key file for authenticating to the remote host. |
| `--remote-xilinx-settings PATH` | Path to `settings64.sh` on the remote host. |
| `--remote-ssh-control-dir DIR` | Local directory for SSH multiplex control sockets. |
| `--remote-ssh-control-persist DURATION` | How long the SSH master socket stays alive (default: `30m`). |
| `--remote-disable-ssh-mux` | Disable SSH connection multiplexing. |

---

## tapa compile

Run the full compilation pipeline (analyze → synth → pack) in a single command. Its flag surface is the union of `tapa analyze`, `tapa synth`, and `tapa pack` — every flag documented for those three is accepted here too.

### Required flags

| Flag | Description |
|------|-------------|
| `--top TASK` / `-t TASK` | Top-level task function name. |
| `--input FILE` / `-f FILE` | Kernel source file. Repeat the flag for multiple sources. |

### Commonly used optional flags

| Flag | Description |
|------|-------------|
| `--output FILE` / `-o FILE` | Output path (default: `work.xo`, or `work.zip` for the `xilinx-hls` target). |
| `--part-num PART` | Target FPGA part number (e.g., `xcu250-figd2104-2L-e`). |
| `--platform PLATFORM` / `-p PLATFORM` | Vitis platform name. Alternative to `--part-num`. |
| `--clock-period NS` | Target clock period in nanoseconds. |
| `--cflags FLAG` / `-c FLAG` | Compiler flag for the kernel (e.g., `-Iinclude`). Repeat the flag for multiple. |
| `--target {xilinx-vitis,xilinx-hls}` | Output target (default: `xilinx-vitis`). |
| `-j N` / `--jobs N` | Number of parallel HLS jobs. |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. Repeatable. |
| `--connectivity FILE` | Vitis link config with memory `sp=` bank assignments. |

### Example

```bash
tapa compile \
  --top VecAdd \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vadd.xo
```

---

## tapa analyze

Parse C++ source and extract the task graph to `tapa.json` in the work directory. This stage always runs locally and does not require vendor tools.

### Required flags

| Flag | Description |
|------|-------------|
| `--top TASK` / `-t TASK` | Top-level task function name. |
| `--input FILE` / `-f FILE` | Kernel source file. Repeat the flag for multiple sources. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--cflags FLAG` / `-c FLAG` | Compiler flag passed through to the kernel frontend, such as an include path. Repeat the flag for multiple. |
| `--target {xilinx-vitis,xilinx-hls}` | Output target (default: `xilinx-vitis`). Controls the synthesis flow. |
| `--flatten-hierarchy` | Flatten the task hierarchy so every leaf task sits directly under the top task. |
| `--keep-hierarchy` | Preserve the task hierarchy as written. This is the default; the flag exists to override `--flatten-hierarchy`. |

### Example

```bash
tapa --work-dir work.out analyze \
  --top VecAdd \
  -f vadd.cpp \
  -c -Iinclude
```

---

## tapa synth

Run Vitis HLS on each task to produce per-task Verilog RTL. Reads the task graph produced by `tapa analyze` from the work directory. Can run on a remote host via `--remote-host`.

### Required flags

| Flag | Description |
|------|-------------|
| `--part-num PART` | Target FPGA part number. Required if `--platform` is not set. |
| `--platform PLATFORM` / `-p PLATFORM` | Vitis platform name. Required if `--part-num` is not set. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--clock-period NS` | Target clock period in nanoseconds. Derived from `--platform` if not set explicitly. |
| `-j N` / `--jobs N` | Number of parallel HLS and post-synthesis jobs (default: the host's available parallelism). |
| `--enable-synth-util` | Run out-of-context Vivado synthesis per task for accurate area numbers instead of the coarser HLS estimates. Costs one extra Vivado run per task. |
| `--keep-hls-work-dir` | Keep each task's Vitis HLS project under `hls/<task>/project` instead of discarding it. Useful for post-mortem debugging of an HLS failure. |
| `--skip-hls-based-on-mtime` | Reuse a task's existing Verilog when it is newer than its extracted C++, skipping that task's HLS run. Speeds up iteration when only some tasks changed. |
| `--other-hls-configs TCL` | Extra Tcl appended verbatim to every generated HLS script. |

### Example

```bash
tapa --work-dir work.out synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -j 4
```

---

## tapa floorplan

Floorplan a design for a multi-die (multi-SLR) FPGA. Run it between `synth` and `pack` to assign tasks to SLRs, balance resource usage across them, and add pipeline registers on channels that cross SLR boundaries. It writes placement and timing constraints (`floorplan.xdc`) that `pack` picks up automatically.

Run it **after** `tapa synth` (it needs each task's resource estimate) and **before** `tapa pack`. The partitioner solves a wire-crossing-minimizing ILP with the local `cbc` solver.

### Supported devices

Floorplanning needs a device table describing the slot grid and where each memory bank and control interface attaches. Tables ship for:

| Device | Part | Banks | Platform the table was built from |
|---|---|---|---|
| Alveo U250 | `xcu250-figd2104-2L-e` | `DDR[0]`–`DDR[3]`, one per SLR | `xilinx_u250_gen3x16_xdma_4_1_202210_1` |
| Alveo U280 | `xcu280-fsvh2892-2L-e` | `HBM[0]`–`HBM[31]`, `DDR[0]`, `DDR[1]` | `xilinx_u280_gen3x16_xdma_1_202211_1` |
| VCK190 | `xcvc1902-vsva2197-2MP-e-S` | `DDR[0]`–`DDR[3]`, by NoC memory controller | `xilinx_vck190_base_202410_1` |

Any other part is rejected with `no floorplan device table matches ...`. The other steps (`analyze`, `synth`, `pack`) work on every part Vitis HLS supports; only floorplanning is restricted.

### Synthesizing with `--platform`, not `--part-num`

If the top task has direct M-AXI ports, run `tapa synth` with `--platform` naming the exact platform in the table above — not with `--part-num`. Floorplanning a design with external memory has to place each M-AXI port next to the shell interface that reaches its bank, and those locations are properties of one specific platform, so the planner refuses to guess:

```
tapa: floorplan error: external-memory floorplanning requires platform
`xilinx_u250_gen3x16_xdma_4_1_202210_1`; rerun synthesis with `--platform`
```

Naming a different platform is rejected the same way, with `platform ... does not match floorplan device platform ...`. `--part-num` alone is enough only for a design with no direct M-AXI ports, where there is nothing to anchor.

This applies to every floorplan run, not just the `--run-impl` and `--dse` flows below — those need `--platform` for a further reason, that they invoke `v++ --link`.

### Optional flags (planning)

| Flag | Description |
|------|-------------|
| `--connectivity FILE` | Vitis link `sp=` config mapping each direct M-AXI port to a memory bank. Required when the kernel has direct M-AXI ports (HBM/DDR pinning) — without it the planner cannot tell which bank a port reaches, and it stops with `floorplanning direct M-AXI ports requires --connectivity with sp=...`, naming every port it wants a line for. See [Connectivity file](#connectivity-file) for the exact syntax. |
| `--usage-limit FRAC` | Per-slot resource utilization target for a non-DSE plan; raised on infeasibility (default `0.7`). |
| `--partition-strategy {auto,flat,multi-level}` | Placement schedule (default `auto`). `flat` places directly into atomic slots with one ILP; `multi-level` places into rows (SLRs) first, then refines into atomic slots. `auto` picks between them with a built-in heuristic. |
| `--pp-scheme {single,double,single_h_double_v}` | How pipeline registers distribute across a crossing's route (default `double`). |
| `--max-seconds N` | ILP wall-clock limit in seconds (default `600`). |

### Connectivity file

A standard Vitis link config, one `sp=` line per direct M-AXI port:

```ini
[connectivity]
sp=VecAdd.a:HBM[0]
sp=VecAdd.b:HBM[1]
sp=VecAdd.c:HBM[2]
```

The left-hand side is `<compute-unit>.<argument>`. TAPA names the compute unit after the top task — its generated bitstream script passes `--connectivity.nk VecAdd:1:VecAdd` — so the name is the bare top task, **not** the `VecAdd_1` form Vitis defaults to when the kernel is instantiated without `nk`. The argument is the top task's parameter name, as it appears in the C++ signature. Bank targets are `HBM[n]` or `DDR[n]`.

The same file is what `tapa pack --connectivity` forwards to `v++ --link` as a `--config`, so one file serves both steps.

### Optional flags (implementation / DSE)

These run the floorplanned design through `v++ --link` to measure Fmax. On top of the `--platform` synthesis that external-memory floorplanning already requires, they need a `xilinx-vitis` target and a platform `v++` still accepts, and they run Vivado/Vitis on the tool host.

| Flag | Description |
|------|-------------|
| `--run-impl` | Plan one candidate at `--usage-limit`, package the XO, and run one `v++ --link` to measure its Fmax. |
| `--dse` | Design-space exploration: sweep exact logic-utilization caps across `--dse-min`…`--dse-max` and keep the highest-frequency implementation. |
| `--dse-min FRAC` / `--dse-max FRAC` / `--dse-step FRAC` | DSE cap range and step (defaults `0.55` / `0.9` / `0.03`; require `--dse`). The sweep starts at `--dse-max` and steps down. |
| `--dse-jobs N` | Max candidate package/link jobs to run concurrently (default `1`; requires `--dse`). |
| `--vivado-threads N` | Per-link Vivado synthesis jobs (`--vivado.synth.jobs`, default `2`). Lower it on memory-constrained hosts. |

### What the floorplan XDC emits

`tapa floorplan` writes `floorplan.xdc`, sourced by `pack`/`v++` as `OPT_DESIGN.TCL.PRE`. It contains:

- **Pblocks** for every slot and pipeline stage (`IS_SOFT 1`, `CONTAIN_ROUTING 0`). On DFX/shell platforms Vivado may force `IS_SOFT=0` for hierarchical-flow children — this is expected and is what drops routing congestion.
- **Pipeline-stage cell matches** (`TAPA_HS_HEAD/BODY/TAIL`) constrained to their route cells, with `USER_SLL_REG` on vertical-boundary bodies.
- **Reset-distribution timing cuts**: `set_false_path` on the reset net (TAPA's `__tapa_control_fabric_reset_n` and the platform `peripheral_aresetn`), both `-quiet`. Reset deassertion is not a per-cycle setup concern; cutting it removes the dominant cross-SLR reset wall.

### Example

```bash
tapa --work-dir work.out synth --platform xilinx_u280_gen3x16_xdma_1_202211_1 --clock-period 3
tapa --work-dir work.out floorplan \
  --partition-strategy multi-level \
  --pp-scheme double \
  --connectivity connectivity.ini \
  --dse --dse-min 0.55 --dse-max 0.90 --dse-step 0.05 \
  --vivado-threads 2
tapa --work-dir work.out pack -o kernel.xo
```

The floorplanned `pack` consumes `floorplan.xdc` and the connectivity config automatically once the floorplan marker is present.

---

## tapa pack

Package per-task RTL from the work directory into a single output artifact. For the default `xilinx-vitis` target this produces an XO file; for other targets a ZIP file is produced. Reads RTL produced by `tapa synth`.

### Optional flags

| Flag | Description |
|------|-------------|
| `--output FILE` / `-o FILE` | Output file path (default: `work.xo` for the Vitis target, `work.zip` for other targets). |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. Repeat the flag for multiple paths. Not available after `tapa floorplan`. |
| `--connectivity FILE` | Memory connectivity `.ini` with the Vitis `sp=` bank assignments. When set, the emitted bitstream script passes it to `v++ --link` as a `--config`, binding each M-AXI port to its bank — required for HBM/DDR designs. |
| `--bitstream-script FILE` / `-s FILE` | Write the generated bitstream-generation script to this path. |

### Example

```bash
tapa --work-dir work.out pack -o vadd.xo
```

---

## tapa g++

Compile TAPA host and kernel C++ for software simulation. This is a wrapper around `g++` that automatically sets `-std=c++17` plus the required TAPA include paths and link flags. Every remaining argument is forwarded to `g++` verbatim.

### Optional flags

| Flag | Description |
|------|-------------|
| `--executable PATH` | Run this compiler instead of `g++` (default: `g++`). |

### Example

```bash
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd
```

The `--` separator is optional — it is only needed when your first forwarded argument could be mistaken for one of `tapa g++`'s own flags. `tapa g++` is terminal: no further subcommand can be chained after it.

See [Software Simulation](../howto/software-simulation.md) for how to run the resulting executable.

---

## tapa version

Print the installed TAPA version.

```bash
tapa version
```
