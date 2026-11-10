# Output Files

TAPA writes a final RTL archive and, by default, keeps intermediate results in
`work.out/`. Use the same work directory for every stage of one design; use a
different directory for independent designs.

## Final artifacts

The `--target` selected during analysis determines the output format.

### `xilinx-vitis` (default)

`tapa pack` produces an `.xo` file. Use it for fast cosimulation or run
`tapa link --package <name>.xo` on it to build an `.xclbin` bitstream. For
an AI Engine design, pack also writes `<name>.libadf.a` and `<name>.aie.cfg`
next to the XO, and `tapa link` links and packages all three. A VCK5000 build
also writes `<name>.aie_control.xo`, a PL kernel that controls the AI Engine
graph through a dedicated PLIO; link includes it in the same hardware image.

A design whose top runs on the AI Engine has no PL kernel, so `tapa pack`
writes a `.zip` instead: `tapa.json`, the graph as `libadf.a`, and
`arguments.json`, the top's stream arguments, which `tapa link` records in
the xclbin for the runtime. Link writes `<name>.xsa` beside the `.xclbin`.
On a VCK5000 such a top gets a PL kernel TAPA builds for its rings, so pack
writes an `.xo` and its sidecars as above.

`tapa compile --aie-sim` writes a directory at `--output` instead: the AI
Engine graph compiled for the simulator, and `manifest.json`, which tells
the runtime how the host's arguments and streams reach it. The host
executable takes the directory as `--bitstream`. See [AI Engine
Tasks](../howto/aie.md).

### `xilinx-slash`

`tapa pack` produces a `.zip` package for SLASH linking and fast
cosimulation. A typical archive contains:

```text
work.zip
├── impl/ip/                 # packaged IP directory (component.xml, HDL, xgui)
├── hls_data.json            # register map and ABI metadata for slashkit
├── kernel.xml               # kernel description for packaging and cosim
├── report.yaml              # synthesis summary
└── tapa.json                # kernel interface and build metadata
```

`tapa link --package <name>.zip` builds the service-shell `.vbin` from it,
and the host executable accepts the same `.zip` as `--bitstream` for fast
cosimulation. See [Alveo V80 with SLASH](../howto/v80.md).

### `xilinx-hls`

`tapa pack` produces a `.zip` file for fast cosimulation or a downstream RTL
flow. A typical archive contains:

```text
work.zip
├── rtl/                    # generated RTL
├── report/TASK/            # per-task HLS reports, when available
├── report.yaml             # synthesis summary, when available
└── tapa.json               # kernel interface and build metadata
```

Every package format, the `.xo` included, is a ZIP archive that you can
inspect with standard archive tools. To replace a task's RTL, prefer the
supported [`--custom-rtl`](cli.md#tapa-pack) workflow over editing a packed
artifact by hand.

## Reproducibility

TAPA removes timestamps, absolute paths, and random IDs from final artifacts.
Given the same sources, compiler options, and tool versions, repeated builds
produce byte-identical output. Vendor-tool upgrades can change the artifact
contents even when your source does not change.

## Work-directory contents

The exact files depend on the selected options:

```text
work.out/
├── rewritten/                 # sources prepared for synthesis
├── hls/TASK/                  # per-task HLS results
│   ├── report/
│   ├── verilog/
│   └── project/               # with --keep-hls-work-dir
├── rtl/                       # RTL prepared for packaging
├── report/                    # detailed utilization, with --enable-synth-util
├── report.json                # synthesis summary
├── report.yaml                # synthesis summary
├── template/                  # custom-RTL module shells, when needed
├── templates_info.json        # custom-RTL port information, when needed
├── tapa.json                  # compiler state shared between stages
├── tapacc.json                # raw frontend output for debugging
├── aie/                       # with AI Engine tasks
│   ├── hw/                    # the AI Engine graph tapa synth compiles
│   └── pl-control/            # the VCK5000 PL controller tapa pack builds
├── floorplan.xdc              # with tapa floorplan
├── floorplan-connectivity.ini # with tapa floorplan --connectivity
├── floorplan-metrics.json     # with --run-impl or --dse
├── floorplan-timing.rpt       # with --run-impl or --dse
├── floorplan.xclbin           # implementation winner, Vitis backend
├── floorplan.vbin             # implementation winner, SLASH backend
├── link/                      # with tapa link (inputs, hook, metrics)
│   ├── link-metrics.json      # with a SLASH link
│   ├── hbm-slices.tsv         # generated HBM slices, SLASH hardware links
│   └── control-coverage.tsv   # high-fanout control requests, SLASH hardware links
└── dse/                       # with --run-impl or --dse
```

### Files to use during development

| Path | What you use it for |
|---|---|
| `hls/TASK/report/` | HLS scheduling, initiation interval, latency, and estimated area for one task. |
| `hls/TASK/project/` | Full Vitis HLS project retained by `--keep-hls-work-dir` for debugging an HLS failure. |
| `report.json` / `report.yaml` | Timing and resource summaries after a successful `tapa synth`. These contain the same data in JSON and YAML formats. |
| `report/` | Detailed per-task utilization reports produced by `--enable-synth-util`. |
| `template/` and `templates_info.json` | Port shells and interface information for a task marked `[[tapa::target("ignore")]]`; used with `--custom-rtl`. |
| `floorplan-metrics.json` | Frequency and utilization information for the selected floorplan after `--run-impl` or `--dse`. |
| `floorplan-timing.rpt` | Post-implementation timing report used to produce those metrics. |
| `floorplan.xclbin` / `floorplan.vbin` | The winning implementation's bitstream from `--run-impl` or `--dse` (`.xclbin` from the Vitis backend, `.vbin` from SLASH). |
| `link/link-metrics.json` | SLASH link record: pinned `slash_commit`, `requested_mhz` vs `recorded_mhz`, kernel WNS and Fmax, `timing_closed`, and the fixed-shell domain's WNS. A `--sim` link records only the commit and requested versus recorded MHz — simulation produces no routed timing report. |
| `link/hbm-slices.tsv` | SLASH hardware link: full AXI register-slice configuration on each HBM interface selected by the connectivity map. |
| `link/control-coverage.tsv` | SLASH hardware link: eligible high-fanout fabric controls requested for clock-region replication. |
| `dse/candidates.json` | Implementation results and ranking diagnostics for every candidate of `--run-impl` or `--dse`; final full-link metrics are in `floorplan-metrics.json`. |

### Files TAPA manages for you

- **`rewritten/`** contains the sources used for synthesis. Regenerate it with
  `tapa analyze` after changing kernel sources instead of editing it manually.
- **`rtl/`** contains the RTL prepared for packaging. Custom RTL overlays are
  applied here by `tapa pack --custom-rtl`.
- **`tapa.json`** carries the task graph and compilation settings between
  stages. Keep it with the rest of the work directory and do not edit it
  manually. If TAPA reports an incompatible version, rerun `tapa analyze`.
- **`tapacc.json`** is raw frontend output retained for diagnostics. Later
  stages do not read it, so editing it has no effect.
- **`floorplan.xdc`** contains the constraints produced by `tapa floorplan`.
  The package does not carry it; pass it, with
  `floorplan-connectivity.ini`, to `tapa link` (`--floorplan-xdc`,
  `--connectivity`). You normally do not need to edit it.

The final archive includes the interface metadata needed by fast cosimulation,
so you can move the `.xo` or `.zip` to another machine without also copying the
work directory.
