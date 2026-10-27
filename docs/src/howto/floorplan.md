# Floorplan for Timing Closure

Use `tapa floorplan` to improve clock frequency on a supported multi-SLR FPGA.
It places tasks, balances their resource use, and adds pipelining where signals
cross SLR boundaries.

Floorplanning is optional. It is most useful when implementation reports show
cross-SLR timing failures or routing congestion. It does not fix incorrect
functionality or a long combinational path inside one task, and it is not
needed when the design already meets timing.

## Prerequisites

- A functionally correct design; run software simulation first.
- A work directory produced by `tapa analyze` and `tapa synth`.
- The `cbc` solver on `PATH` for planning. See
  [Install CBC](../start/installation.md#install-cbc).
- A [supported device and platform](../reference/cli.md#supported-devices).
- For a kernel with direct M-AXI ports: synthesis with the exact supported
  `--platform`, plus a connectivity file assigning every port to a memory bank.
- For `--run-impl` or `--dse`: the `xilinx-vitis` or `xilinx-slash` target,
  and the matching toolchain — Vitis/Vivado with a platform the installed
  `v++` accepts, or SLASH's `slashkit` with the V80 static shell for
  `slash_v80_service` (see [Alveo V80 with SLASH](v80.md)).

For tighter designs, consider `tapa synth --enable-synth-util` before
floorplanning. It provides more accurate per-task resource estimates at the
cost of an additional Vivado synthesis run per task.

## Basic workflow

`tapa compile` does not run the floorplan step. Run the stages separately so
you can insert it between synthesis and packaging:

```bash
tapa --work-dir work.out analyze \
  --top VecAdd \
  -f vadd.cpp

tapa --work-dir work.out synth \
  --platform xilinx_u55c_gen3x16_xdma_3_202210_1 \
  --clock-period 3.33

tapa --work-dir work.out floorplan \
  --connectivity connectivity.ini

tapa --work-dir work.out pack \
  -o vecadd.xo
```

If you already ran `tapa compile` in this work directory, analysis and
synthesis are complete: run only `floorplan` followed by `pack`. If you change
sources or rerun `synth`, run `floorplan` again before `pack`.

## Assign memory banks

For this example, `connectivity.ini` assigns VecAdd's three direct M-AXI
parameters to separate HBM banks. On the V80 the same role is played by
slashkit's own configuration (`nk=`/`sp=`), described in
[Alveo V80 with SLASH](v80.md#the-slash-configuration):

```ini
[connectivity]
sp=VecAdd.a:HBM[0]
sp=VecAdd.b:HBM[1]
sp=VecAdd.c:HBM[2]
```

Use `<top-task>.<parameter>` with the names from your C++ signature. For a
`tapa::mmaps<T, N>` parameter, use indices such as `Bandwidth.chan[0]`.
Provide one assignment for every direct M-AXI port. A design without direct
M-AXI ports does not need this file.

## Start with the defaults

The default options are a good first plan:

- `--partition-strategy auto` selects the placement strategy.
- `--pp-scheme double` controls pipelining across SLR crossings.
- `--usage-limit 0.7` is the initial per-slot resource target; the planner may
  raise it when the target is infeasible.
- `--max-seconds 600` limits each planning solve, not the total runtime of the
  whole floorplan step.

If pipelining consumes too many resources, try `--pp-scheme single`. If a
planning solve needs more time, increase `--max-seconds`. See the
[`tapa floorplan` CLI reference](../reference/cli.md#tapa-floorplan) for all
options.

## Package and validate

`floorplan` writes `floorplan.xdc`. For `xilinx-vitis`, `pack` applies that
file and the staged connectivity configuration automatically, so no extra
flags are required.

Pipelining can add latency across SLR boundaries but should not change the
results. Run fast cosimulation again to check functionality. Fast cosim does
not measure achieved Fmax; use the implementation timing report or on-board
measurements for that.

## Measure a plan with `--run-impl`

Planning alone does not prove that the design closes timing. To package one
plan and run the device's implementation backend (`v++ --link` on U-series,
SLASH on V80) for feedback:

```bash
tapa --work-dir work.out floorplan \
  --connectivity connectivity.ini \
  --run-impl \
  --vivado-threads 2
```

Review:

- `work.out/floorplan-metrics.json` for the selected plan's utilization and
  achieved frequency.
- `work.out/floorplan-timing.rpt` for the post-implementation timing details.
- `work.out/floorplan.xclbin` (Vitis) or `work.out/floorplan.vbin` (SLASH),
  the implemented bitstream of the measured plan.

If implementation fails inside one task's combinational logic, change the HLS
source rather than increasing floorplan effort. Add pipelining or split the
operation across cycles, then rerun synthesis and floorplanning.

## Explore plans with `--dse`

Design-space exploration sweeps utilization targets and keeps the candidate
with the highest measured frequency. The ranking backend is the device's own:
`v++ --link` on U-series boards, SLASH on the V80:

```bash
tapa --work-dir work.out floorplan \
  --connectivity connectivity.ini \
  --dse \
  --dse-min 0.55 \
  --dse-max 0.90 \
  --dse-step 0.05 \
  --dse-jobs 2 \
  --vivado-threads 2
```

The sweep starts at `--dse-max` and moves toward `--dse-min`.
`--dse-jobs` limits concurrent package/link jobs; `--vivado-threads` controls
Vivado synthesis parallelism within each job. Increasing either can
substantially increase memory use. Review `dse/candidates.json` for the full
sweep and `floorplan-metrics.json` for the winner.

## Remote execution

Planning itself runs locally and needs local `cbc`. With `--remote-host`,
`--run-impl` and `--dse` can run their Vitis/Vivado implementation jobs on the
remote machine. See [Remote Execution](remote-execution.md).

## Common problems

| Problem | What to check |
|---|---|
| `no floorplan device table matches ...` | Use a supported device family and, for direct M-AXI, its exact platform. |
| External memory requires a platform | Rerun `synth` with the exact supported `--platform`, then rerun `floorplan`. |
| Connectivity is missing or incomplete | Add one `sp=` assignment per direct M-AXI port and check top-task and parameter spelling. |
| Planning is infeasible | Review per-task area, try `--pp-scheme single`, and consider `--enable-synth-util` for better estimates. |
| Planning times out | Increase `--max-seconds`; it applies per solve rather than to the whole step. |
| Timing still fails within a task | Restructure that task in HLS. Floorplanning addresses placement and routing, not logic depth. |
| Results differ after floorplanning | Rerun fast cosim. Then check custom RTL and protocol correctness; floorplanning should add latency, not change values. |
