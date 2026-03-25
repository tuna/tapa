# Lab 6: Floorplan & DSE

**Goal:** Use TAPA's floorplan design space exploration (DSE) to achieve timing closure on multi-SLR FPGAs.

**Prerequisites:** [Lab 2: High-Bandwidth Memory](lab-02-async-mmap.md) and familiarity with synthesis flags from [Performance Tuning](../howto/performance-tuning.md).

After this lab you will understand how to apply a floorplan solution to a compile step and, if the RapidStream optimization tool is available, how to generate floorplan solutions automatically.

---

## Overview

Multi-SLR FPGAs (U250, U280, U55C, and similar) partition logic across physically separate silicon dies connected by SLR crossings. Long wires that cross SLR boundaries are a common source of timing failures. TAPA's floorplan tooling addresses this by:

- Assigning tasks to specific SLR regions.
- Automatically inserting pipeline registers on streams that cross SLR boundaries.
- Running a design space exploration to find placement configurations that stay within per-SLR resource limits.

---

## Tool dependency

The floorplan generation step — which searches for optimal task-to-SLR assignments — requires **`rapidstream-tapaopt`**, an optimization tool historically provided by RapidStream Design Automation. **This tool is no longer publicly accessible.** If you hold a license, the full two-workflow process described below applies. If you do not, you can still apply a hand-written or externally provided `floorplan.json` directly using Workflow A Step 2, skipping the generation step.

```admonish note
Compiling a design with a floorplan applied — inserting pipeline registers and reorganizing the task hierarchy — works without `rapidstream-tapaopt`. Only the automated search for floorplan solutions requires the external tool.
```

---

## Workflow A: Manual floorplan

Use this workflow when you want to inspect individual floorplan solutions before committing to a full compile, or when you already have a `floorplan.json` from another source.

### Step 1: Generate floorplan solutions *(requires `rapidstream-tapaopt`)*

```bash
tapa generate-floorplan \
  -f kernel.cpp \
  -t kernel0 \
  --device-config device_config.json \
  --floorplan-config floorplan_config.json \
  --clock-period 3.00 \
  --part-num xcu55c-fsvh2892-2L-e
```

This runs the DSE and writes one or more `floorplan_N.json` files to the working directory. Each file represents a distinct placement solution.

### Step 2: Compile with a chosen solution

```bash
tapa compile \
  -f kernel.cpp \
  -t kernel0 \
  --floorplan-path floorplan_0.json \
  --clock-period 3.00 \
  --part-num xcu55c-fsvh2892-2L-e \
  --flatten-hierarchy
```

```admonish warning
`--floorplan-path` requires `--flatten-hierarchy`. Omitting `--flatten-hierarchy` will cause the compile to fail.
```

TAPA reorganizes the task hierarchy according to the chosen floorplan and inserts pipeline registers at all SLR-crossing streams. This step does **not** require `rapidstream-tapaopt`.

---

## Workflow B: Automated DSE *(requires `rapidstream-tapaopt`)*

Use this workflow to generate and compile all floorplan solutions in one step without manual inspection between them.

```bash
tapa compile-with-floorplan-dse \
  -f kernel.cpp \
  -t kernel0 \
  --device-config device_config.json \
  --floorplan-config floorplan_config.json \
  --clock-period 3.00 \
  --part-num xcu55c-fsvh2892-2L-e
```

`compile-with-floorplan-dse` runs the DSE, then compiles and applies pipeline insertion for each floorplan solution it generates. Use this when you want to produce all candidates in one automated run and pick the best result based on downstream timing reports.

---

## Floorplan config format

The `--floorplan-config` JSON controls how the DSE searches for placement solutions. A representative example:

```json
{
  "max_seconds": 1000,
  "dse_range_min": 0.7,
  "dse_range_max": 0.88,
  "partition_strategy": "flat",
  "cpp_arg_pre_assignments": {
    "a": "SLOT_X1Y0:SLOT_X1Y0",
    "b_0": "SLOT_X2Y0:SLOT_X2Y0"
  },
  "sys_port_pre_assignments": {
    "ap_clk": "SLOT_X2Y0:SLOT_X2Y0"
  }
}
```

Key fields:

- `dse_range_min` / `dse_range_max` — The acceptable per-SLR resource utilization range (as a fraction of 1.0). The DSE only keeps placements where every SLR falls within this band.
- `cpp_arg_pre_assignments` — Forces specific top-function kernel arguments to specific SLR slots. Values are `SLOT_XmYn:SLOT_XmYn` strings. Array arguments can be matched with regex patterns (for example `"c_.*"` matches `c_0`, `c_1`, etc.).
- `sys_port_pre_assignments` — Forces Verilog system ports (clock, reset, AXI control) to specific slots. Regex patterns are supported here as well.

The full set of available fields (including `grouping_constraints`, `slot_to_rtype_to_min_limit`, and others) is documented in the RapidStream floorplan configuration reference.

---

## Further reading

[Performance Tuning](../howto/performance-tuning.md) covers the `--gen-ab-graph` and `--gen-graphir` flags, which produce visual and structural representations of the task graph useful for diagnosing floorplan decisions.

---

**Next step:** [Examples Catalog](examples-catalog.md)
