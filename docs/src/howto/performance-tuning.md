# Performance Tuning

**Purpose:** Identify and fix throughput bottlenecks in your TAPA design.

**When to use this:** When your design builds and runs correctly but measured throughput is below your target — for example, the kernel time is higher than expected or resource utilization is unexpectedly high.

## What you need

- A compiled `.xo` from `tapa --work-dir work.out compile ...`
- Reports in `work.out/` (synthesis reports, utilization data)
- Understanding of your design's expected throughput

## Prioritized checklist

Work through these checks in order — each is faster to fix than the next.

### 1. Check initiation interval (II) in synthesis reports

After `tapa compile`, check the HLS reports in `work.out/` for II violations:

- An II > 1 on a pipelined loop means it cannot accept a new iteration every
  cycle, which can reduce throughput.
- Look for scheduling warnings and the achieved II in the HLS report.

If the loop is not already pipelined, add `[[tapa::pipeline(1)]]` to request
II=1. If it still reports II>1, read the violation and restructure the
specified dependency, memory access, or data path. The attribute is a request,
not a guarantee that HLS can achieve II=1.

### 2. Check memory throughput — consider `async_mmap`

`mmap` leaves request scheduling to HLS and does not expose separate
request/response channels. If memory latency limits your task:

- Use `tapa::async_mmap` to overlap computation and memory access.
- Check the synthesis report for memory interface utilization.

### 3. Check stream depths — FIFOs too shallow?

A shallow FIFO can throttle a bursty producer even when the consumer's
average rate is sufficient. First rule out an element-count or protocol bug;
increasing depth cannot fix a structural deadlock. Then:

- Check waveforms from fast cosim (`-xsim_save_waveform`) to observe
  backpressure.
- If additional burst storage is needed, increase the depth in
  `tapa::stream<T, DEPTH>` and review the resource cost.

### 4. Find resource hotspots with `--enable-synth-util`

Run synthesis with utilization reporting enabled:

```bash
tapa --work-dir work.out synth \
  --enable-synth-util \
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33
```

TAPA runs an additional out-of-context Vivado synthesis pass per task on the
generated RTL and writes per-task LUT, FF, BRAM, URAM, and DSP counts to:

- `work.out/report.json` — machine-readable JSON
- `work.out/report.yaml` — human-readable YAML

Use them to identify which tasks are consuming the most resources before
proceeding to full implementation. If these files are missing, synthesis
either did not run or exited before the reporting step; check the HLS log in
`work.out/` for errors.

The extra pass matters because the default per-task estimates are read from
the Vitis HLS synthesis reports, and **HLS estimates are coarse and
frequently inaccurate** — they tend to undercount control and interconnect
logic. The post-RTL-synthesis numbers are materially more accurate, so
prefer them whenever the estimates matter — before floorplanning a tight
design or when interpreting the utilization report — at the cost of one
extra Vivado synth per task.

## Improving Fmax with `tapa floorplan`

The checks above improve throughput. To improve clock frequency on a
supported multi-SLR device, run `tapa floorplan` between `synth` and `pack`.
It places tasks, balances resources, and pipelines SLR crossings.

Use floorplanning for cross-SLR timing failures or routing congestion, not for
long combinational paths inside a task. Those require HLS changes.

See [Floorplan for Timing Closure](floorplan.md) for the complete workflow,
including connectivity, `--run-impl`, DSE, result files, and troubleshooting.

## If something goes wrong

```admonish warning
See [Common Errors](../troubleshoot/common-errors.md) for help with synthesis failures, II violation messages, and resource overflows.
```

---

**Next step:** [Learning Path](../tutorials/learning-path.md)
