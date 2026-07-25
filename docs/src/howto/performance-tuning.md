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

## Improving Fmax with `tapa floorplan`

The checks above address *throughput* (II, memory, FIFO depth). To improve *clock frequency* (Fmax) on a multi-die (multi-SLR) device, run `tapa floorplan` between `synth` and `pack`. It partitions the design across SLRs via a wire-crossing-minimizing ILP, inserts relay pipeline registers on cross-slot channels, and writes pblock + timing constraints.

### When it helps

A design is **SLR-crossing-bound** when, without a floorplan, Vivado cannot route it at the target clock (global congestion level 7 = unroutable) or leaves long cross-die paths. On such designs the floorplan is load-bearing: it can turn an unroutable 3 ns design into a routable ~300 MHz one. On designs that already meet timing, a floorplan is unnecessary.

### How to measure (implementation / DSE)

```bash
tapa --work-dir work.out floorplan \
  --partition-strategy multi-level --pp-scheme double \
  --connectivity connectivity.ini \
  --dse --dse-min 0.55 --dse-max 0.90 --dse-step 0.05 \
  --vivado-threads 2
```

`--dse` sweeps exact logic-utilization caps and runs each through `v++ --link`, reporting the achieved Fmax per candidate. `--run-impl` runs a single candidate at `--usage-limit` instead. The winning candidate's metrics land in `work.out/floorplan-metrics.json`; per-candidate diagnostics in `work.out/dse/candidates.json`.

### What still bounds Fmax after floorplanning

Floorplanning relieves **placement/routing** pressure (SLR crossing, congestion) and **reset-distribution** (the emitted XDC cuts the cross-SLR reset net out of setup timing). It does **not** change intra-task logic depth. If the worst failing paths are inside a single task — e.g. a 9-level float-compare → RAM-read datapath scheduled by HLS in one cycle — no floorplan change will close timing; that requires an HLS pipelining directive (`#pragma HLS pipeline` / loop restructuring) on the offending task. Use the routed timing report's *Intra Clock Table* (the kernel-clock WNS row) and the `report_timing` worst paths to tell the two apart: cross-SLR or reset-source paths are floorplan-addressable; intra-task paths are HLS-addressable.

## Advanced flags summary

| Flag | Description |
|------|-------------|
| `--enable-synth-util` | Run post-HLS RTL synthesis to collect per-task resource utilization. |
| `--disable-synth-util` | Do not run post-HLS RTL synthesis (default). |

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for help with synthesis failures, II violation messages, and resource overflows.
```

---

**Next step:** [Learning Path](../tutorials/learning-path.md)
