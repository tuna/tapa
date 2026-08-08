# Examples Catalog

The TAPA repository includes two sets of example designs. Small self-contained examples live under `tests/apps/`. Larger benchmarks live under `tests/regression/`.

---

## Small examples

These are the complete contents of `tests/apps/`. Each one builds and runs
under software simulation with no FPGA and no vendor tools.

| Example | Problem type | Key TAPA feature | Location |
|---------|-------------|-----------------|----------|
| vadd | Vector addition | Basic streams + mmap | `tests/apps/vadd` |
| async\_mmap | Vector addition, decoupled memory | `async_mmap` request/response channels | `tests/apps/async_mmap` |
| bandwidth | Memory bandwidth benchmark | `async_mmap` over many HBM channels | `tests/apps/bandwidth` |
| cannon | Cannon's matrix multiply | 2D stream arrays, systolic | `tests/apps/cannon` |
| gemv | Matrix-vector multiply | Stream reduction | `tests/apps/gemv` |
| graph | Graph traversal | Large per-task local buffers | `tests/apps/graph` |
| ignore | Custom-RTL placeholder | `[[tapa::target("ignore")]]` | `tests/apps/ignore` |
| jacobi | Stencil computation | End-of-transmission (`close()`) | `tests/apps/jacobi` |
| network | Packet switching | `peek`, detached tasks, hierarchical tasks | `tests/apps/network` |
| templated | Parameterized kernels | Templated leaf tasks | `tests/apps/templated` |

```admonish tip
`tests/functional/` holds smaller single-feature designs used as regression
tests — `shared-mmap`, `detached`, `eot`, `peek`, `custom-rtl`,
`parallel-emulation`, and others. They are the shortest working reference
for one specific feature.
```

---

## Published benchmarks

These live under `tests/regression/` and are full-scale designs from
published papers.

| Example | Problem type | Key feature | Published in |
|---------|-------------|-------------|-------------|
| autosa | Matrix multiplication | AutoSA-generated systolic (90% U55C LUT) | — |
| callipepla | Conjugate gradient | 26 HBM channels | [FPGA'23](https://dl.acm.org/doi/pdf/10.1145/3543622.3573182) |
| cnn | CNN systolic array | Multi-SLR | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| lu\_decompose | LU systolic array | Multi-SLR | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| hbm-bandwidth | HBM bandwidth profiler | `async_mmap`, all 32 channels | — |
| hbm-bandwidth-1-ch | HBM bandwidth (1 channel) | Minimal `async_mmap` | — |
| serpens-16ch / -24ch / -32ch | Sparse SpMV | Same architecture at three parallelism levels | [DAC'22](https://arxiv.org/pdf/2111.12555.pdf) |
| spmm | Sparse SpMM | HBM streams | [FPGA'22](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357) |
| spmv-hisparse-mmap | Sparse SpMV (HiSparse) | mmap-based SpMV | [FPGA'22](https://www.csl.cornell.edu/~zhiruz/pdfs/spmv-fpga2022.pdf) |
| stencil-dilate | Stencil / dilation | Multi-stage stencil pipeline | — |
| knn | K-nearest-neighbor | FPT accelerator | [FPT'20](http://www.sfu.ca/~zhenman/files/C19-FPT2020-CHIP-KNN.pdf) |
| page\_rank | Page Rank | FCCM accelerator | [FCCM'21](https://about.blaok.me/pub/fccm21-tapa.pdf) |

```admonish note
These are large designs: they need Vitis HLS and are meant for evaluating
frequency and resource usage, not for learning the API. Start with
`tests/apps/` for that. New designs are added over time — check
`tests/regression/` in the repository for the current list.
```

---

**Next step:** [Common Errors](../troubleshoot/common-errors.md)
