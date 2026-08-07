# Output Files

## Output Artifacts

The artifact produced by `tapa` depends on the target selected with `--target`.

**Xilinx Vitis target** (`--target xilinx-vitis`, the default)

Produces an `.xo` object file. This is passed to the Vitis `v++` compiler for bitstream generation. An XO file is a ZIP archive; you can unzip it to inspect or manually edit the RTL it contains, then re-zip it before passing it to `v++`.

**Xilinx HLS target** (`--target xilinx-hls`)

Produces a `.zip` RTL archive instead of an `.xo` file. The archive contains the same RTL files and metadata but without the Vitis shell wrapper. Use this when the RTL is consumed directly by a downstream EDA tool.

The archive layout is:

```text
work.zip
├── rtl/                    # the generated RTL tree
├── report/TASK/            # per-task HLS `_csynth.rpt` files
├── report.yaml             # when `tapa synth` emitted one
└── tapa.json               # compile metadata (see below)
```

`tapa.json` is a verbatim copy of the work directory's [`tapa.json`](#file-and-directory-descriptions) — same schema, same bytes. It is what lets a consumer recover the kernel's interface from the archive alone: TAPA's own co-simulation runtime reads the top task's ports out of `graph` to bind kernel arguments, and takes the part number from `flow`.

## Reproducibility

TAPA strips timestamps, absolute paths, and random IDs from both `.xo` and `.zip` artifacts before writing them to disk. Given the same source code and tool versions, repeated invocations produce byte-identical output. This makes the artifacts suitable for CI and release attestation workflows.

```admonish note
Byte identity holds only within the same vendor tool version. Upgrading Vitis HLS or Vivado will typically change internal artifact content even for identical source inputs.
```

## Intermediate Files

When `--work-dir` is specified (recommended), TAPA writes intermediate files to that directory. The structure is:

```text
work.out/
├── cpp/
├── flatten/
├── hls/
│   └── TASK/
│       ├── report/
│       ├── verilog/
│       └── project/        # with --keep-hls-work-dir
├── rtl/
├── report/                    # with --enable-synth-util
├── template/                  # when a task targets "ignore"
├── tapa.json
├── tapacc.json
├── templates_info.json        # when a task targets "ignore"
├── report.json
└── report.yaml
```

### File and directory descriptions

**`cpp/`**

Contains per-task C++ source files extracted by `tapa analyze`. Each file is independently compiled to RTL by `vitis_hls`.

**`flatten/`**

Created during `tapa analyze`. Contains preprocessed (flattened) copies of the input source files, one per input file, with a short hash prefix in the filename to avoid collisions. All `#include` directives are expanded and comments are preserved, giving `tapacc` self-contained translation units to operate on.

**`hls/`**

Contains one directory per synthesized task. Each task directory has the HLS reports under `report/` and the harvested Verilog under `verilog/`. Passing `--keep-hls-work-dir` also retains the complete Vitis project under `project/` for debugging.

**`rtl/`**

Contains the complete generated RTL tree: harvested HLS modules, connected upper-task wrappers, control modules, and TAPA infrastructure RTL.

**`report/`**

Created by `--enable-synth-util`. Contains per-task `<task>.hier.util.rpt` files produced by out-of-context Vivado synthesis.

**`template/` / `templates_info.json`**

For tasks annotated with `[[tapa::target("ignore")]]`, `template/` contains generated Verilog module shells for implementing replacement RTL. `templates_info.json` records the expected port metadata used when checking `--custom-rtl` overlays.

**`tapa.json`**

The single state file: everything the pipeline persists between steps, and the only file `tapa` reads back. It is pretty-printed so work directories stay readable and diffable. It has three top-level keys:

- **`version`** — schema version of this file. `tapa` refuses a work directory stamped with a version it does not recognise, telling you to re-run `tapa analyze` rather than failing with an obscure parse error.
- **`graph`** — all contents and metadata of the input design, including the task graph structure. Written by `tapa analyze` from the `tapacc` output, with the flow `target` and the kernel `cflags` added. `tapa synth` then annotates each task in place with its post-synthesis `clock_period`, `self_area`, and `total_area`; those fields are absent until synthesis populates them.
- **`flow`** — compilation settings shared across pipeline steps (part number, clock period, platform, and whether synthesis has run), resolved by `tapa synth` and read back by `tapa pack` so options need not be repeated on the command line. The flow `target` is deliberately not duplicated here: it lives in `graph.target` alone.

```admonish note
`clock_period` appears in both blocks and means different things: `flow.clock_period` is the clock period you *requested*, while a task's `graph.tasks.<TASK>.clock_period` is the period HLS *estimated* for it. Both are whole picoseconds — `3330` is 3.33 ns — so a period is one number rather than text a reader has to parse. `report.json` prints nanoseconds, since that is what its readers recognise.
```

**`tapacc.json`**

The raw, unmodified output of `tapacc`, saved verbatim by `tapa analyze` for provenance and debugging — including when it fails to parse. Nothing in the pipeline reads it back; `tapa.json` is the only state. Edits to this file have no effect.

**`report.json` / `report.yaml`**

Timing and resource-utilisation report, written unconditionally after `tapa synth` completes. Both files contain the same data in JSON and YAML encoding. Without `--enable-synth-util`, areas come from HLS estimates; with it, child-task totals are replaced by out-of-context Vivado utilization and the underlying `.hier.util.rpt` files are written under `report/`.
