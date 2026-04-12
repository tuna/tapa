# Performance Tuning

**Purpose:** Identify and fix throughput bottlenecks in your TAPA design.

**When to use this:** When your design builds and runs correctly but measured throughput is below your target — for example, the kernel time is higher than expected or resource utilization is unexpectedly high.

## What you need

- A compiled `.xo` from `tapa compile --work-dir work.out`
- Reports in `work.out/` (synthesis reports, utilization data)
- Understanding of your design's expected throughput

## Prioritized checklist

Work through these checks in order — each is faster to fix than the next.

### 1. Check initiation interval (II) in synthesis reports

After `tapa compile`, check the HLS reports in `work.out/` for II violations:

- An II > 1 on a pipelined loop means the loop is not fully pipelined and throughput is reduced.
- Look for `WARNING: [HLS ...] Unable to schedule` or `II = N` where N > 1 in the HLS log.

Fix: Add `#pragma HLS pipeline II=1` or restructure the loop body to eliminate data-path dependencies.

### 2. Check memory throughput — consider `async_mmap`

Synchronous `mmap` accesses stall the task until each memory transaction completes. If your task spends time waiting for DRAM:

- Use `tapa::async_mmap` to overlap computation and memory access.
- Check the synthesis report for memory interface utilization.

### 3. Check stream depths — FIFOs too shallow?

FIFOs that are too shallow cause backpressure and reduce throughput when producer and consumer tasks run at different rates. If tasks are frequently stalling:

- Increase the stream depth in your TAPA source: `tapa::stream<T, DEPTH>`.
- Check waveforms from fast cosim (`-xsim_save_waveform`) to observe backpressure.

### 4. Find resource hotspots with `--enable-synth-util`

Run synthesis with utilization reporting enabled:

```bash
tapa --work-dir work.out synth \
  --enable-synth-util \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33
```

TAPA runs an additional RTL synthesis pass and writes per-task resource counts to:

- `work.out/report.json` — machine-readable JSON
- `work.out/report.yaml` — human-readable YAML

Both files contain per-task LUT, FF, BRAM, and DSP counts. Use them to identify which tasks are consuming the most resources before proceeding to full implementation.

## Validation

After running `tapa synth --enable-synth-util`, confirm the reports were written:

```bash
ls work.out/report.json work.out/report.yaml
```

- `work.out/report.json` — machine-readable per-task resource counts (LUT, FF, BRAM, DSP)
- `work.out/report.yaml` — human-readable version of the same data

If these files are missing, synthesis either did not run or exited before the reporting step. Check the HLS log in `work.out/` for errors.

## Advanced synthesis flags

### Controlling FIFO pipelining for floorplanning

By default, TAPA inserts pipeline registers into stream FIFOs to improve timing. When grouping FIFOs with their adjacent logic inside a single floorplan region, suppress pipelining for specific FIFOs:

```bash
tapa synth --nonpipeline-fifos fifos.json ...
```

`fifos.json` lists the FIFO names to suppress:

```json
["fifo_a", "fifo_b"]
```

After synthesis, TAPA writes `grouping_constraints.json` to the work directory. Pass this file to RapidStream or other floorplanning tools.

### AutoBridge graph generation

Generate an `ab_graph.json` for AutoBridge/RapidStream partition-based floorplanning:

```bash
tapa synth \
  --gen-ab-graph \
  --floorplan-config floorplan.json \
  ...
```

`--floorplan-config` is required when `--gen-ab-graph` is used. It specifies the target device floorplan regions.

### GraphIR generation

Produce a GraphIR representation for RapidStream:

```bash
tapa synth \
  --gen-graphir \
  --device-config device.json \
  --floorplan-path floorplan.json \
  ...
```

Both `--device-config` and `--floorplan-path` are required:

| Flag | Description |
|------|-------------|
| `--device-config PATH` | JSON file describing the physical device (SLR layout, DSP column positions, etc.) |
| `--floorplan-path PATH` | Floorplan assignment file applied to the program before GraphIR is emitted |

The output is `work.out/graphir.json`, suitable for consumption by RapidStream.

## Advanced flags summary

| Flag | Description |
|------|-------------|
| `--enable-synth-util` | Run post-HLS RTL synthesis to collect per-task resource utilization. |
| `--disable-synth-util` | Do not run post-HLS RTL synthesis (default). |
| `--nonpipeline-fifos <json>` | Suppress pipeline registers for listed FIFOs; write `grouping_constraints.json`. |
| `--gen-ab-graph` | Generate `ab_graph.json` for AutoBridge/RapidStream floorplanning. Requires `--floorplan-config`. |
| `--floorplan-config PATH` | Device floorplan region description. Required with `--gen-ab-graph`. |
| `--gen-graphir` | Generate `graphir.json` for RapidStream. Requires `--device-config` and `--floorplan-path`. |
| `--device-config PATH` | Physical device description for GraphIR conversion. Required with `--gen-graphir`. |
| `--floorplan-path PATH` | Floorplan assignment applied before GraphIR emission. Required with `--gen-graphir`. |

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for help with synthesis failures, II violation messages, and resource overflows.
```

---

**Next step:** [Learning Path](../tutorials/learning-path.md)
