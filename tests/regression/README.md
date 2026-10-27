<!--
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
-->

# Regression designs

This directory contains full-scale TAPA designs used for regression testing
and performance evaluation. For small learning examples, start with
`tests/apps/`; for focused feature tests, see `tests/functional/`.

## Included designs

- `autosa`, `cnn`, and `lu_decompose`: AutoSA-generated systolic arrays
  published in [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292).
- `callipepla`: a mixed-precision conjugate-gradient solver using 26 HBM
  channels.
- `hbm-bandwidth` and `hbm-bandwidth-1-ch`: asynchronous memory benchmarks
  using 32 HBM channels and one HBM channel, respectively.
- `knn`: a K-nearest-neighbors accelerator published in
  [FPT'20](http://www.sfu.ca/~zhenman/files/C19-FPT2020-CHIP-KNN.pdf).
- `page_rank`: a PageRank accelerator included in
  [FCCM'21](https://about.blaok.me/pub/fccm21-tapa.pdf).
- `serpens-16ch`, `serpens-24ch`, and `serpens-32ch`: variants of the Serpens
  sparse matrix-vector architecture published in
  [DAC'22](https://arxiv.org/pdf/2111.12555.pdf) with different parallelism.
- `spmm`: Sextans sparse-matrix dense-matrix multiplication variants published
  in [FPGA'22](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357).
- `spmv-hisparse-mmap`: a HiSparse sparse matrix-vector variant published in
  [FPGA'22](https://www.csl.cornell.edu/~zhiruz/pdfs/spmv-fpga2022.pdf).
- `stencil-dilate`: a multi-channel image-dilation stencil benchmark.

See the [examples catalog](https://tapa.readthedocs.io/en/latest/tutorials/examples-catalog.html)
for feature summaries and links to the corresponding publications.

## Manual V80 recipes

The vadd and Callipepla V80 flows use the same target suffixes. Every target
is `manual`; none is part of the nightly regression or an automatic CI tier.

| Recipe | vadd target | Callipepla target |
|---|---|---|
| Compile the 3.33 ns `xilinx-slash` package | `//tests/apps/vadd:vadd-v80-zip` | `//tests/regression:callipepla-v80-zip` |
| Run TAPA fast cosim on the `.zip` | `//tests/apps/vadd:vadd-v80-zipsim` | `//tests/regression:callipepla-v80-zipsim` |
| Build the SLASH `.sim.vbin` | `//tests/apps/vadd:vadd-v80-sim-vbin` | `//tests/regression:callipepla-v80-sim-vbin` |
| Run the host with the `.sim.vbin` | `//tests/apps/vadd:vadd-v80-sim` | `//tests/regression:callipepla-v80-sim` |
| Build the hardware `.vbin` and `link-metrics.json` | `//tests/apps/vadd:vadd-v80-vbin` | `//tests/regression:callipepla-v80-vbin` |
| Run the board hand-off suite | `//tests/apps/vadd:vadd-v80-board` | `//tests/regression:callipepla-v80-board` |

The vadd board suite checks lengths 1, 7, 16, 4097, 65537, and 13.
Callipepla runs `spd-14.mtx 16`. Hardware link outputs are written below the
corresponding Bazel package as `<name>.vbin` and
`<name>-vbin/link-metrics.json`.
