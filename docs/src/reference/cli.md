# CLI Commands

Reference for all `tapa` CLI subcommands. For task-oriented guides, see [Build and Run](../howto/build-and-run.md) and the other How-To pages. The general invocation form is:

```
tapa [global options] <subcommand> [subcommand options]
```

```admonish note
`tapa compile` is a shortcut that runs `tapa analyze`, `tapa synth`, and `tapa pack` in sequence in a single command. When using the individual subcommands, pass `--work-dir` as a **global** flag before the subcommand name: `tapa --work-dir DIR <subcommand>`.
```

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

Run the full compilation pipeline (analyze → synth → pack) in a single command.

### Required flags

| Flag | Description |
|------|-------------|
| `--top FUNCTION` / `-t FUNCTION` | Top-level task function name. |
| `-f FILE` | Kernel source file. |
| `-o OUTPUT.xo` | Output XO file path. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--part-num PART` | Target FPGA part number (e.g., `xcu250-figd2104-2L-e`). |
| `--platform PLATFORM` | Vitis platform string. Alternative to `--part-num`. |
| `--clock-period NS` | Target clock period in nanoseconds. |
| `--target {xilinx-vitis,xilinx-hls}` | Output target (default: `xilinx-vitis`). |
| `-j N` | Number of parallel HLS jobs. |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. |

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

Parse C++ source and extract the task graph to a JSON file in the work directory. This stage always runs locally and does not require vendor tools.

### Required flags

| Flag | Description |
|------|-------------|
| `--top FUNCTION` / `-t FUNCTION` | Top-level task function name. |
| `-f FILE` | Kernel source file. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--target {xilinx-vitis,xilinx-hls}` | Output target (default: `xilinx-vitis`). Controls the synthesis flow. |

### Example

```bash
tapa --work-dir work.out analyze --top VecAdd -f vadd.cpp
```

---

## tapa synth

Run Vitis HLS on each task to produce per-task Verilog RTL. Reads the task graph produced by `tapa analyze` from the work directory. Can run on a remote host via `--remote-host`.

### Required flags

| Flag | Description |
|------|-------------|
| `--part-num PART` | Target FPGA part number. Required if `--platform` is not set. |
| `--platform PLATFORM` | Vitis platform string. Required if `--part-num` is not set. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--clock-period NS` | Target clock period in nanoseconds. Can be derived from `--platform` if not set explicitly. |
| `-j N` | Number of parallel HLS and post-synthesis jobs (default: available logical CPU count). |
| `--enable-synth-util` | Run post-HLS RTL synthesis to produce per-task resource utilization estimates. |

### Example

```bash
tapa --work-dir work.out synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -j 4
```

---

## tapa floorplan

Coarse-grained floorplanning for multi-die (multi-SLR) FPGAs, slotting between `synth` and `pack`. It partitions the flattened task graph into physical SLR slots via a wire-crossing-minimizing ILP (solved with the local `cbc` binary), inserts latency-insensitive relay/handshake pipeline registers on every cross-slot channel, and writes pblock constraints (`floorplan.xdc`) plus a `FloorplanResult` into the work state. The presence of the floorplan marker switches later `pack` and codegen onto the floorplanned path.

Run it **after** `tapa synth` (it needs the per-task areas) and **before** `tapa pack`.

### Required flags

| Flag | Description |
|------|-------------|
| `--connectivity FILE` | Vitis link `sp=` config mapping each direct M-AXI port to a memory bank. Required when the kernel has direct M-AXI ports (HBM/DDR pinning). |

### Optional flags (planning)

| Flag | Description |
|------|-------------|
| `--usage-limit FRAC` | Per-slot resource utilization target for a non-DSE plan; raised on infeasibility (default `0.7`). |
| `--partition-strategy {auto,flat,multi-level}` | Placement schedule. `multi-level` places into rows (SLRs) then refines into atomic slots. |
| `--pp-scheme {single,double,single_h_double_v}` | How pipeline registers distribute across a crossing's route (default `double`). |
| `--max-seconds N` | ILP wall-clock limit in seconds (default `600`). |

### Optional flags (implementation / DSE)

These run the floorplanned design through `v++ --link` to measure Fmax. They require synthesis with `--platform <installed-platform-name>` and a `xilinx-vitis` target, and they run Vivado/Vitis on the tool host.

| Flag | Description |
|------|-------------|
| `--run-impl` | Plan one candidate at `--usage-limit`, package the XO, and run one `v++ --link` to measure its Fmax. |
| `--dse` | Design-space exploration: sweep exact logic-utilization caps across `--dse-min`…`--dse-max`. |
| `--dse-min FRAC` / `--dse-max FRAC` / `--dse-step FRAC` | DSE cap range and step (require `--dse`). |
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
| `-o OUTPUT` | Output file path (default: `work.xo` for the Vitis target, `work.zip` for other targets). |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. |

### Example

```bash
tapa --work-dir work.out pack -o vadd.xo
```

## tapa g++

Compile TAPA host and kernel C++ for software simulation. This is a wrapper around `g++` that automatically sets the required TAPA include paths and link flags. All arguments after `--` are forwarded directly to `g++`.

### Example

```bash
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd
```

See [Software Simulation](../howto/software-simulation.md) for how to run the resulting executable.

---

## tapa version

Print the installed TAPA version.

```bash
tapa version
```
