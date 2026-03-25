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
| `--target {xilinx-vitis,xilinx-hls,xilinx-aie}` | Output target (default: `xilinx-vitis`). `xilinx-aie` is experimental. |
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

### Optional flags

| Flag | Description |
|------|-------------|
| `--target {xilinx-vitis,xilinx-hls,xilinx-aie}` | Output target (default: `xilinx-vitis`). Controls the synthesis flow. `xilinx-aie` is experimental. |

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

### Optional flags

| Flag | Description |
|------|-------------|
| `-o OUTPUT.xo` | Output XO file path (default: `work.xo` in the work directory). |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the XO. |

### Example

```bash
tapa --work-dir work.out pack -o vadd.xo
```

---

## tapa cosim

Low-level command that runs a fast hardware cosimulation session using pre-generated config and testbench files. In typical usage, `tapa cosim` is invoked internally by the fast cosim host executable (e.g., `./vadd --bitstream=vadd.xo`); advanced users who need to call it directly can use the flags below. See [Fast Hardware Simulation](../howto/fast-cosim.md) for details.

### Required flags

| Flag | Description |
|------|-------------|
| `--config-path PATH` | Path to the cosim config JSON file generated by the host executable. |
| `--tb-output-dir PATH` | Directory containing the generated testbench files. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--part-num PART` | Target FPGA part number. |
| `--launch-simulation / --no-launch-simulation` | Whether to launch the simulation after setup (default: no-launch). |
| `--save-waveform / --no-save-waveform` | Whether to save waveform output (default: no-save). |
| `--start-gui / --no-start-gui` | Whether to open the simulator GUI (default: no-start). |
| `--simulator {xsim,verilator}` | Simulator backend (default: `xsim`). `xsim` requires Vivado and runs on Linux only. `verilator` is cross-platform and does not require Vivado. |

### Example

```bash
# Typical user-facing workflow — the host executable calls tapa cosim internally:
./vadd --bitstream=vadd.xo

# Advanced: invoke tapa cosim directly with pre-generated files:
tapa cosim \
  --config-path work.out/cosim_config.json \
  --tb-output-dir work.out/testbench \
  --launch-simulation
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
