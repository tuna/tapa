# Output Files

TAPA writes a final RTL archive and, by default, keeps intermediate results in
`work.out/`. Use the same work directory for every stage of one design; use a
different directory for independent designs.

## Final artifacts

The `--target` selected during analysis determines the output format.

### `xilinx-vitis` (default)

`tapa pack` produces an `.xo` file. Use it for fast cosimulation or run
`tapa link --package <name>.xo` on it to build an `hw.xclbin` bitstream. For
an AI Engine design, pack also writes `<name>.libadf.a` and `<name>.aie.cfg`
next to the XO, and `tapa link` links and packages all three.

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

Both formats are ZIP-based archives that you can inspect with standard archive
tools. To replace a task's RTL, prefer the supported
[`--custom-rtl`](cli.md#tapa-pack) workflow over editing a packed artifact by
hand.

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
├── floorplan.xdc              # with tapa floorplan
├── floorplan-metrics.json     # with --run-impl or --dse
├── floorplan-timing.rpt       # with --run-impl or --dse
└── dse/                       # with tapa floorplan --dse
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
| `dse/candidates.json` | Summary of all candidates explored by `--dse`. |

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
  `tapa pack` applies it automatically; you normally do not need to edit it.

The final archive includes the interface metadata needed by fast cosimulation,
so you can move the `.xo` or `.zip` to another machine without also copying the
work directory.
