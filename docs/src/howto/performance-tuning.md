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
  --part-num xcu55c-fsvh2892-2L-e \
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

### Where per-task resource estimates come from

TAPA needs a resource estimate for each task to balance the design across SLRs during floorplanning and to report utilization. There are two sources, in order of increasing accuracy:

- **HLS estimates (always available).** After Vitis HLS synthesizes a task, TAPA reads its synthesis report for LUT, FF, BRAM, DSP, and URAM counts. These come "for free" with every `tapa synth` run, but **HLS estimates are coarse and frequently inaccurate** — they tend to undercount control and interconnect logic. Treat them as a rough guide, not a placement-quality number.

- **Post-RTL-synthesis estimates (with `--enable-synth-util`).** This runs an additional out-of-context Vivado synthesis pass per task on the generated RTL, producing materially more accurate resource counts. **Prefer this whenever the estimates matter** — i.e. before floorplanning a tight design or when interpreting the utilization report. It costs one extra Vivado synth per task.

When `--enable-synth-util` has run, `report.json`/`report.yaml` mark `area.source: "synth"`; otherwise `area.source: "hls"`.

## Improving Fmax with `tapa floorplan`

The checks above improve *throughput* (II, memory, FIFO depth). To improve *clock frequency* (Fmax) on a multi-SLR device, run `tapa floorplan` between `synth` and `pack`. It assigns tasks to SLRs, balances resource usage across them, adds pipeline registers on the channels that cross SLR boundaries, and generates the placement and timing constraints for `pack`.

### When it helps

Floorplanning helps when your design is **SLR-crossing-bound**: without it, Vivado either fails to route at your target clock or leaves long, slow paths across dies. Typical signs are routing congestion or worst paths that hop between SLRs. If your design already meets timing, you don't need it.

### Running it with implementation feedback

`tapa floorplan` can plan alone (fast — just writes constraints), or run the plan through `v++ --link` to measure the resulting Fmax:

```bash
tapa --work-dir work.out floorplan \
  --partition-strategy multi-level --pp-scheme double \
  --connectivity connectivity.ini \
  --dse --dse-min 0.55 --dse-max 0.90 --dse-step 0.05 \
  --vivado-threads 2
```

- `--run-impl` plans once and measures that single plan.
- `--dse` sweeps a range of per-SLR utilization caps, measures each, and reports the best Fmax. Results land in `floorplan-metrics.json` (winner) and `dse/candidates.json` (all candidates).

See the [`tapa floorplan` CLI reference](../reference/cli.md) for the full flag set.

### What floorplanning can and cannot fix

Floorplanning fixes problems caused by **placement and routing** — SLR crossings, congestion, and reset-distribution delay. It does **not** change the logic inside a task. If your worst failing paths live entirely within one task (a long combinational chain the HLS scheduler placed in a single cycle), no floorplan will close timing; you need to restructure that task in HLS (e.g. add a `#pragma HLS pipeline`, split the operation across cycles).

To tell the two apart, read the post-implementation timing report: worst paths whose *source and destination sit in different SLRs*, or whose source is a reset/clock tree, are placement problems the floorplan can address; worst paths contained inside a single task instance are logic-depth problems that require HLS changes.

## Advanced flags summary

| Flag | Description |
|------|-------------|
| `--enable-synth-util` | Run post-HLS RTL synthesis to collect per-task resource utilization. |

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for help with synthesis failures, II violation messages, and resource overflows.
```

---

**Next step:** [Learning Path](../tutorials/learning-path.md)
