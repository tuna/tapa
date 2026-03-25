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
| `--verbose` / `-v` | Increase logging verbosity. Repeatable (e.g., `-vv`). |
| `--quiet` / `-q` | Decrease logging verbosity. |
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
| `-f FILE` | Kernel source file. Repeatable to include multiple files. |
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
| `-f FILE` | Kernel source file. Repeatable. |

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
| `--clock-period NS` | Target clock period in nanoseconds. |

### Optional flags

| Flag | Description |
|------|-------------|
| `-j N` | Number of parallel HLS jobs (default: 1). |
| `--enable-synth-util` | Run post-HLS RTL synthesis to produce per-task resource utilization estimates. |
| `--nonpipeline-fifos JSON` | JSON specification of FIFOs for which pipeline registers should be suppressed. |
| `--gen-ab-graph` | Generate `ab_graph.json` for AutoBridge/RapidStream floorplanning. |
| `--gen-graphir` | Generate `graphir.json` for RapidStream. |

### Example

```bash
tapa --work-dir work.out synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -j 4
```

---

## tapa pack

Package per-task RTL from the work directory into a single XO file. Reads RTL produced by `tapa synth`.

### Required flags

| Flag | Description |
|------|-------------|
| `-o OUTPUT.xo` | Output XO file path. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. |

### Example

```bash
tapa --work-dir work.out pack -o vadd.xo
```

---

## tapa cosim

Run fast hardware cosimulation using an XO file produced by `tapa compile` or `tapa pack`. See [Fast Hardware Simulation](../howto/fast-cosim.md) for details.

### Optional flags

| Flag | Description |
|------|-------------|
| `--simulator {xsim,verilator}` | Simulator backend (default: `xsim`). `xsim` requires Vivado and runs on Linux only. `verilator` is cross-platform and does not require Vivado. |
| `--xo PATH` | Path to the XO file. |

### Example

```bash
tapa cosim --simulator verilator --xo vadd.xo
```

---

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
