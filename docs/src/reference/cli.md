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
| `--quiet` / `-q` | Decrease logging verbosity. Repeatable. |
| `--remote-host user@host[:port]` | Remote Linux host where vendor tools run. |
| `--remote-key-file PATH` | SSH private key file for authenticating to the remote host. |
| `--remote-xilinx-settings PATH` | Path to `settings64.sh` on the remote host. |
| `--remote-ssh-control-dir DIR` | Local directory for SSH multiplex control sockets. |
| `--remote-ssh-control-persist DURATION` | How long the SSH master socket stays alive (default: `30m`). |
| `--remote-disable-ssh-mux` | Disable SSH connection multiplexing. |

---

## tapa compile

Run analyze, synth, and pack in a single command. It accepts the options
documented for those three stages.

### Required flags

| Flag | Description |
|------|-------------|
| `--top TASK` / `-t TASK` | Top-level task function name. |
| `--input FILE` / `-f FILE` | Kernel source file; each flag is one translation unit. Repeat for multi-file programs. |

### Commonly used optional flags

| Flag | Description |
|------|-------------|
| `--output FILE` / `-o FILE` | Output path (default: `work.xo`, or `work.zip` for the `xilinx-hls` and `xilinx-slash` targets). |
| `--part-num PART` | Target FPGA part number (e.g., `xcu250-figd2104-2L-e`). |
| `--platform PLATFORM` / `-p PLATFORM` | Vitis platform name. Alternative to `--part-num`. |
| `--clock-period NS` | Target clock period in nanoseconds. |
| `--cflags FLAG` / `-c FLAG` | Compiler flag for the kernel (e.g., `-Iinclude`). Repeat the flag for multiple. |
| `--target {xilinx-vitis,xilinx-slash,xilinx-hls}` | Output target (default: `xilinx-vitis`). Also takes `TASK:TARGET` mappings, see `analyze`. |
| `-j N` / `--jobs N` | Number of parallel HLS jobs. |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the output archive. Repeatable. |

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

Parse C++ sources and record the task graph in the work directory. Repeat
`--input` for multi-file programs; all files use the same compiler flags.
This stage always runs locally and does not require vendor tools. See
[Multi-File Designs](../howto/multi-file.md) for a complete example.

### Required flags

| Flag | Description |
|------|-------------|
| `--top TASK` / `-t TASK` | Top-level task function name. |
| `--input FILE` / `-f FILE` | Kernel source file; each flag is one translation unit. Repeat for multi-file programs. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--cflags FLAG` / `-c FLAG` | Compiler flag passed through to the kernel frontend, such as an include path. Repeat the flag for multiple. |
| `--target {xilinx-vitis,xilinx-slash,xilinx-hls}` | Output target (default: `xilinx-vitis`). Controls the synthesis and linking flow; `xilinx-slash` builds for the SLASH service shell, see [Alveo V80 with SLASH](../howto/v80.md). |
| `--target TASK:TARGET` | Build task `TASK` as `aie`, `hls` or `ignore`, overriding its `[[tapa::target]]`. Repeatable; an unknown task is an error. See [AI Engine Tasks](../howto/aie.md). |
| `--flatten-hierarchy` | Emit flattened RTL while preserving task behavior. The stored task graph remains hierarchical. |
| `--keep-hierarchy` | Preserve the task hierarchy. This is the default. |


### Example

```bash
tapa --work-dir work.out analyze \
  --top VecAdd \
  -f vadd.cpp \
  -c -Iinclude
```

### Vendor-usage warnings

`analyze` emits soft `remark:` diagnostics when a program uses vendor-specific constructs — vendor headers (`ap_int.h`, `hls_stream.h`, …), `#pragma HLS …`, or wait intrinsics like `ap_wait()`. Each remark names the portable TAPA form:

```
vadd.cpp:5:13: remark: '#pragma HLS pipeline' is vendor-specific; the portable TAPA form is [[tapa::pipeline(II)]]
```

The usage diagnostics are remarks, not errors; unrelated syntax or semantic
errors can still reject the program. The remarks point to the vendor-agnostic
spelling (see the [API reference](api.md)).

---

## tapa synth

Run Vitis HLS using the task graph produced by `tapa analyze` and generate
the design's RTL, or rebuild only its AI Engine graph with `--aie-only`.
This stage can run on a remote host via `--remote-host`.

### Required flags

| Flag | Description |
|------|-------------|
| `--part-num PART` | Target FPGA part number. Required if `--platform` is not set. |
| `--platform PLATFORM` / `-p PLATFORM` | Vitis platform name or directory. Required if `--part-num` is not set. A name is looked up under `platforms/` of `/opt/xilinx` and `$XILINX_VITIS`, and under `$XILINX_VITIS/base_platforms/` for the base platforms Vitis ships (for example `xilinx_vck190_base_202520_1`). The built-in SLASH shells resolve from a typed table instead of a platform directory: `slash_v80_service` builds part `xcv80-lsva4737-2MHP-e-S` at a 3.33 ns default clock and requires the `xilinx-slash` target, which in turn accepts no other platform. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--clock-period NS` | Target clock period in nanoseconds. Derived from `--platform` if not set explicitly. |
| `--skip-aie` | Build only the PL half of a design with AI Engine tasks, without compiling the AI Engine graph; the `.xo` then serves hybrid cosimulation. |
| `--aie-only` | Build only the hardware AI Engine graph. Requires an AI Engine partition and conflicts with `--skip-aie`; does not run HLS, RTL codegen, post-synthesis utilization, or PL timing reporting. |
| `-j N` / `--jobs N` | Number of parallel HLS and post-synthesis jobs (default: the host's available parallelism). |
| `--enable-synth-util` | Run out-of-context Vivado synthesis per task for accurate area numbers instead of the coarser HLS estimates. Costs one extra Vivado run per task. |
| `--keep-hls-work-dir` | Keep each task's Vitis HLS project under `hls/<task>/project` instead of discarding it. Useful for post-mortem debugging of an HLS failure. |
| `--skip-hls-based-on-mtime` | Reuse existing Verilog when the kernel sources and headers are unchanged. Useful for iterating on other parts of a design. |
| `--require-hls-cache` | With `--skip-hls-based-on-mtime`, fail on a cache miss or unusable cached csynth report instead of running HLS or substituting defaults. |
| `--other-hls-configs TCL` | Extra Tcl appended verbatim to every generated HLS script. |

An AIE-only build preserves synthesized PL artifacts and floorplan state only
when the resolved part number, platform, and requested clock period all match
the previous PL synthesis. Changing any of them clears `flow.synthed` and the
floorplan, requiring a full synth before packing; running `analyze` also leaves
the state unsynthesized, and AIE-only never promotes it. The graph's existing
PL metrics remain stored in `tapa.json`.

With valid synthesized PL state and existing root reports, AIE-only rewrites
those reports from the preserved PL metrics and the new AIE mapping. This drops
`aie.aiesim`, so rerun `tapa aiesim --cycle-accurate` for timing of the new
graph. It does not create or refresh root reports without valid synthesized PL
state, nor create them when none exist; the AIE compiler output and console
remain available. Reports invalidated by `analyze` or changed PL inputs stay
stale and are not treated as current.

### Example

```bash
tapa --work-dir work.out synth \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -j 4
```

---

## tapa floorplan

Floorplan a design for a supported multi-die (multi-SLR) FPGA. Run it after
`synth` and before `pack` to place tasks, balance resource usage, and pipeline
connections between SLRs. It writes `floorplan.xdc`, which `pack` applies
automatically. See [Floorplan for Timing Closure](../howto/floorplan.md) for
the guided workflow.

### Supported devices

Floorplanning needs a device table describing the slot grid and where each memory bank and control interface attaches. Tables ship for:

| Device | Reference part | Banks | Platform the table was built from |
|---|---|---|---|
| Alveo U55C | `xcu55c-fsvh2892-2L-e` | `HBM[0]`–`HBM[31]`, no DDR | `xilinx_u55c_gen3x16_xdma_3_202210_1` |
| Alveo U250 | `xcu250-figd2104-2L-e` | `DDR[0]`–`DDR[3]`, one per SLR | `xilinx_u250_gen3x16_xdma_4_1_202210_1` |
| Alveo U280 | `xcu280-fsvh2892-2L-e` | `HBM[0]`–`HBM[31]`, `DDR[0]`, `DDR[1]` | `xilinx_u280_gen3x16_xdma_1_202211_1` |
| Alveo V80 (SLASH service shell) | `xcv80-lsva4737-2MHP-e-S` | `HBM[0]`–`HBM[63]`, `DDR[0]`–`DDR[3]` | `slash_v80_service` |
| VCK190 | `xcvc1902-vsva2197-2MP-e-S` | `DDR[0]`–`DDR[3]`, by NoC memory controller | `xilinx_vck190_base_202410_1` |

Floorplanning also accepts other part strings in these same device families;
an unknown family is rejected with `no floorplan device table matches ...`.
The other steps (`analyze`, `synth`, `pack`) work on parts supported by Vitis
HLS; only floorplanning has this restricted device table.

Following TAPA's coarse-grained floorplan model (TRETS 2023, Section 4.1), this table uses one slot per physical SLR in a 1-column by 3-row grid. Its logic and DSP `area` values represent the capacity exposed to the SLASH reconfigurable module after applying `pblock_slash`; BRAM/URAM site counts are retained as inventory only and are not floorplan limits on V80. Vivado decides BRAM/URAM placement inside the SLASH shell. SLR0 has no TAPA RM placement region in the current shell. HBM ports are anchored to SLR2, while DDR and control ports are anchored to SLR1. The V80 XDC retains the SLR/NOC and logic/DSP ranges, omits explicit BRAM/URAM site ranges, and clips generated slot ranges to `pblock_slash`. The two SLR boundaries each have an 18,870-SLL capacity in the routed shell report.

The 64 `HBM[n]` tags name SLASH AXI endpoints (`HBM_AXI_00`–`HBM_AXI_63`); they are not a claim that V80 has 64 HBM pseudo-channels. AVED describes 32 pseudo-channels, while the exact endpoint-to-pseudo-channel pairing is a property of the installed SLASH service shell. AVED places the HBM_NMUs across the top of SLR2. The RM-side HBM terminal is therefore anchored in SLR2. The service-shell DDR and control terminals are anchored in the usable SLR1 RM slot; this keeps them out of static SLR0 and avoids an unnecessary SLR1-to-SLR2 hop. This table models the RM-side slot anchor and SLR crossing budget; it does not model HNoC/VNoC/NPS routes, NoC bandwidth, or per-bank NoC locality. Vivado/SLASH implements and times those hard-NoC paths.

### Synthesizing with `--platform`, not `--part-num`

When the top task has direct M-AXI ports, synthesize with the exact
`--platform` shown in the table rather than `--part-num`. Memory-bank
placement is platform-specific, and a different platform is rejected.
`--part-num` is sufficient only when there are no direct M-AXI ports.

The `--run-impl` and `--dse` flows also require the `xilinx-vitis` or
`xilinx-slash` target and a platform accepted by the installed `v++` or
SLASH.

### Optional flags (planning)

| Flag | Description |
|------|-------------|
| `--connectivity FILE` | Vitis link `sp=` config mapping each direct M-AXI port to a memory bank. Required when the kernel has direct M-AXI ports (HBM/DDR pinning) — without it the planner cannot tell which bank a port reaches, and it stops with `floorplanning direct M-AXI ports requires --connectivity with sp=...`, naming every port it wants a line for. See [Connectivity file](#connectivity-file) for the exact syntax. |
| `--usage-limit FRAC` | Per-slot resource utilization target for a non-DSE plan; raised on infeasibility (default `0.7`). |
| `--partition-strategy {auto,flat,multi-level}` | Placement strategy (default `auto`, recommended for most designs). |
| `--pp-scheme {single,double,single_h_double_v}` | Pipeline-register distribution across SLR crossings (default `double`). |
| `--max-seconds N` | Time limit for one planning solve in seconds (default `600`). |

### Connectivity file

A standard Vitis link config, one `sp=` line per direct M-AXI port:

```ini
[connectivity]
sp=VecAdd.a:HBM[0]
sp=VecAdd.b:HBM[1]
sp=VecAdd.c:HBM[2]
```

The left-hand side is `<top-task>.<parameter>`, using the names from your
C++ signature. Use the bare top-task name, such as `VecAdd`, without a numeric
suffix. Bank targets are `HBM[n]` or `DDR[n]`.

For a `tapa::mmaps<T, N>` parameter, use bracket indices for the channels:

```ini
[connectivity]
sp=Bandwidth.chan[0]:DDR[0]
sp=Bandwidth.chan[1]:DDR[1]
```

The same `sp=` file feeds floorplanning and hardware linking; `tapa link`
passes it to `v++ --link` for U-series devices. On V80, write slashkit's own
configuration instead (`nk=`, `sp=<instance>.m_axi_x:HBMn`, optional
`[clock]`); see [Alveo V80 with SLASH](../howto/v80.md#the-slash-configuration).

### Optional flags (implementation / DSE)

These run the floorplanned design through the device's implementation backend to measure Fmax. U-series devices use `v++ --link`; V80 uses SLASH to build the service-shell `.vbin`. They require a `xilinx-vitis` or `xilinx-slash` TAPA target, the exact platform used at synthesis, and Vivado plus either Vitis or SLASH (`slashkit`) on the tool host.

| Flag | Description |
|------|-------------|
| `--run-impl` | Plan one candidate at `--usage-limit`, package the XO, and implement it with Vitis or SLASH to measure its Fmax. |
| `--dse` | Design-space exploration: sweep exact logic-utilization caps across `--dse-min`…`--dse-max` and keep the highest-frequency implementation. |
| `--dse-min FRAC` / `--dse-max FRAC` / `--dse-step FRAC` | DSE cap range and step (defaults `0.55` / `0.9` / `0.03`; require `--dse`). The sweep starts at `--dse-max` and steps down. |
| `--dse-jobs N` | Max candidate package/link jobs to run concurrently (default `1`; requires `--dse`). |
| `--vivado-threads N` | Per-link Vivado synthesis jobs (`--vivado.synth.jobs`, default `2`). Lower it on memory-constrained hosts. |

### Example

```bash
tapa --work-dir work.out synth --platform xilinx_u55c_gen3x16_xdma_3_202210_1 --clock-period 3
tapa --work-dir work.out floorplan \
  --partition-strategy multi-level \
  --pp-scheme double \
  --connectivity connectivity.ini \
  --dse --dse-min 0.55 --dse-max 0.90 --dse-step 0.05 \
  --vivado-threads 2
tapa --work-dir work.out pack -o kernel.xo
```

A published floorplan stages `floorplan.xdc` and the connectivity config in
the work directory; pass them to `tapa link` (`--floorplan-xdc`,
`--connectivity`) when building the bitstream.

---

## tapa pack

Package RTL from the work directory into a single output artifact: an XO for
the default `xilinx-vitis` target, or a ZIP for `xilinx-hls` and
`xilinx-slash`. Reads RTL produced by `tapa synth`. A design with AI Engine
tasks also gets `<name>.libadf.a` and `<name>.aie.cfg` next to the XO; see
[AI Engine Tasks](../howto/aie.md). On `xilinx-slash`, the ZIP carries the
packaged IP directory, `kernel.xml`, and `hls_data.json` for slashkit, and
fast cosim loads it unchanged. The packaged artifact feeds `tapa link`,
which produces the `.xclbin` (or `.vbin`) bitstream.

### Optional flags

| Flag | Description |
|------|-------------|
| `--output FILE` / `-o FILE` | Output file path (default: `work.xo` for the Vitis target, `work.zip` for the other targets). |
| `--custom-rtl PATH` | Custom RTL file or directory to include in the output archive. Repeat the flag for multiple paths. Not available after `tapa floorplan`. |

### Example

```bash
tapa --work-dir work.out pack -o vadd.xo
```

---

## tapa link

Link a packaged TAPA kernel into a hardware or simulation bitstream. The
package's recorded flow target picks the backend: `xilinx-vitis` runs
`v++ --link` (and `v++ --package` with AI Engine sidecars), `xilinx-slash`
runs SLASH's `slashkit` against the service shell. Both run through the tool
runner, so they execute on the `--remote-host` machine when one is configured.
`xilinx-hls` has no link backend. See [Build & Run on Board](../howto/build-and-run.md)
and [Alveo V80 with SLASH](../howto/v80.md) for the guided workflows.

### Required flags

| Flag | Description |
|------|-------------|
| `--package FILE` | Packaged TAPA kernel: `.xo` for `xilinx-vitis`, `.zip` for `xilinx-slash`. |
| `--output FILE` / `-o FILE` | Bitstream output: `.xclbin` for `xilinx-vitis`, `.vbin` for `xilinx-slash`. |
| `--connectivity FILE` | Connectivity configuration. Required for `xilinx-slash` (slashkit's `nk=`/`sp=` file); for `xilinx-vitis`, the standard `sp=` config passed to `v++ --config`. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--clock-mhz MHZ` | Requested kernel clock. Defaults to the clock period the package's flow was synthesized with. A SLASH `[clock]` section in the config must match this value; TAPA appends one when absent. |
| `--floorplan-xdc FILE` | Floorplan constraints (`tapa floorplan`'s `floorplan.xdc`) applied during implementation. |
| `-j N` / `--jobs N` | Vivado parallelism per link (`--vivado.synth.jobs` for Vitis, `--jobs` for slashkit; default `8`). |
| `--sim` | Produce a SLASH simulation vbin instead of a hardware image (`xilinx-slash` only). |
| `--vpp PROGRAM` | Vitis linker/packager on the tool host (default `v++`; Vitis links only). |
| `--slashkit PROGRAM` | SLASH linker on the tool host (default `slashkit`; SLASH links only). |
| `--vivado PROGRAM` | Vivado on the tool host (default `vivado`; SLASH links only). |

A SLASH link succeeds whenever slashkit produces a vbin: a missed kernel
clock derates the recorded frequency with a warning instead of failing, and
`link-metrics.json` records requested versus recorded MHz, the kernel WNS,
and the pinned SLASH commit. A Vitis link reports the routed kernel timing
from its post-route report either way. A `--sim` link has no routed timing
report — its metrics record the commit and the requested versus recorded MHz
only.

### Example

```bash
tapa --work-dir work.out link --package vadd.zip \
  --connectivity v80.cfg -o vadd.vbin
```

---

## tapa aiesim

Run the AI Engine tasks of an analyzed design in `x86simulator`, or in the
cycle-approximate `aiesimulator`, against the boundary streams a software
simulation recorded, and compare what the graph writes back. See
[AI Engine Tasks](../howto/aie.md).

### Required flags

| Flag | Description |
|------|-------------|
| `--stimulus DIR` | Directory of `<stream>.words` files from a software simulation run with `TAPA_STREAM_LOG_DIR=DIR`. |

### Optional flags

| Flag | Description |
|------|-------------|
| `--arg NAME=VALUE` | Value of a top-task scalar the AI Engine tasks read. Repeatable. |
| `--cycle-accurate` | Compile for hardware and run `aiesimulator`, reporting when each output stream's beats arrive; when every output matches, that timing is also folded into the `aie.aiesim` section of the existing `report.json`/`report.yaml` (no report is created when `tapa synth` has not run). Needs the AI Engine compiler licence. |
| `--platform PLATFORM` / `-p PLATFORM` | Vitis platform; defaults to the one `tapa synth` used. |
| `--part-num PART` | Part number, when no platform is known. |
| `--cycles N` | AI Engine cycles a never-returning graph runs before it is ended (default 100000). |

### Example

```bash
TAPA_STREAM_LOG_DIR=logs ./vadd-host 256
tapa --work-dir work.out aiesim --stimulus logs --arg n=256 \
  --part-num xcvc1902-vsva2197-2MP-e-S
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

---

## tapa update

Update TAPA to the latest release, replacing the current installation in
place. `tapa upgrade` is an alias. You need write permission for the
installation directory; use `sudo` for a system-wide install.

```bash
tapa update
```

Separately, `tapa` checks for new releases automatically: at most once every 24 hours it fetches the latest release tag in the background (a detached process that never blocks or fails your command), caches the result, and prints a warning at the end of the **next** invocation when a newer release exists. Set `TAPA_NO_UPDATE_CHECK=1` to disable the check.

```admonish note
The automatic check requests only
`https://api.github.com/repos/tuna/tapa/releases/latest` and writes its cache
under `$XDG_CACHE_HOME/tapa/` (or `~/.cache/tapa/`). It does not send project
sources or design data; the HTTPS request itself still goes to GitHub.
```
