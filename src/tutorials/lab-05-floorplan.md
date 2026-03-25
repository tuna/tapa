# Lab 5: Floorplan & DSE

**Goal:** Use TAPA's floorplan design space exploration (DSE) to achieve timing closure on multi-SLR FPGAs.

**Prerequisites:** [Lab 2: async\_mmap](lab-02-async-mmap.md) and familiarity with synthesis flags from [Performance Tuning](../howto/performance-tuning.md).

After this lab you will understand how to generate floorplan solutions, apply them to a compile step, and use the automated DSE command to run both phases together.

---

## Overview

Multi-SLR FPGAs (U250, U280, U55C, and similar) partition logic across physically separate silicon dies connected by SLR crossings. Long wires that cross SLR boundaries are a common source of timing failures. TAPA's floorplan tooling addresses this by:

- Assigning tasks to specific SLR regions.
- Automatically inserting pipeline registers on streams that cross SLR boundaries.
- Running a design space exploration to find placement configurations that stay within per-SLR resource limits.

Two workflows are available: a **manual workflow** that separates floorplan generation from compilation, and an **automated DSE workflow** that runs both in one command.

---

## Workflow A: Manual floorplan

Use this workflow when you want to inspect individual floorplan solutions before committing to a full compile.

### Step 1: Generate floorplan solutions

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
  --device-config device_config.json \
  --floorplan-path floorplan_0.json \
  --clock-period 3.00 \
  --part-num xcu55c-fsvh2892-2L-e \
  --flatten-hierarchy
```

```admonish warning
`--floorplan-path` requires `--flatten-hierarchy`. Omitting `--flatten-hierarchy` will cause the compile to fail.
```

TAPA reorganizes the task hierarchy according to the chosen floorplan and inserts pipeline registers at all SLR-crossing streams.

---

## Workflow B: Automated DSE

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

The full set of available fields (including `grouping_constraints`, `slot_to_rtype_to_min_limit`, and others) is documented in the RapidStream floorplan configuration reference. Refer to the RapidStream documentation for details.

---

## Further reading

[Performance Tuning](../howto/performance-tuning.md) covers the `--gen-ab-graph` and `--gen-graphir` flags, which produce visual and structural representations of the task graph useful for diagnosing floorplan decisions.

---

**Next step:** [Examples Catalog](examples-catalog.md)
