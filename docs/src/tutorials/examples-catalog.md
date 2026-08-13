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

## Full-scale benchmarks

These live under [`tests/regression/`](https://github.com/tuna/tapa/tree/main/tests/regression).
Most are full-scale artifacts from published papers; the repository-only
benchmarks are described below the table.

| Example | Problem type | Key feature | Published in |
|---------|-------------|-------------|-------------|
| [autosa](https://github.com/tuna/tapa/tree/main/tests/regression/autosa) | Matrix multiplication | AutoSA-generated systolic array for U250 and U55C | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| [callipepla](https://github.com/tuna/tapa/tree/main/tests/regression/callipepla) | Conjugate gradient | Mixed-precision solver over 26 HBM channels | [FPGA'23](https://dl.acm.org/doi/pdf/10.1145/3543622.3573182) |
| [cnn](https://github.com/tuna/tapa/tree/main/tests/regression/cnn) | CNN systolic array | Multi-SLR AutoSA design | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| [lu_decompose](https://github.com/tuna/tapa/tree/main/tests/regression/lu_decompose) | LU decomposition | Multi-SLR AutoSA design | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| [knn](https://github.com/tuna/tapa/tree/main/tests/regression/knn) | K-nearest neighbors | 18-way HBM search | [FPT'20](http://www.sfu.ca/~zhenman/files/C19-FPT2020-CHIP-KNN.pdf) |
| [page_rank](https://github.com/tuna/tapa/tree/main/tests/regression/page_rank) | PageRank | HBM graph processing with replicated tasks | [FCCM'21](https://about.blaok.me/pub/fccm21-tapa.pdf) |
| [serpens-16ch](https://github.com/tuna/tapa/tree/main/tests/regression/serpens-16ch), [24ch](https://github.com/tuna/tapa/tree/main/tests/regression/serpens-24ch), [32ch](https://github.com/tuna/tapa/tree/main/tests/regression/serpens-32ch) | Sparse SpMV | Same Serpens architecture at three HBM parallelism levels | [DAC'22](https://arxiv.org/pdf/2111.12555.pdf) |
| [Sextans U55C](https://github.com/tuna/tapa/tree/main/tests/regression/spmm/sextans-u55c-3x3floorplan), [split BRAM/URAM](https://github.com/tuna/tapa/tree/main/tests/regression/spmm/sextans-u280-split-bram-uram) | Sparse SpMM | Streaming HBM architecture with fixed and runtime dimensions | [FPGA'22](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357) |
| [spmv-hisparse-mmap](https://github.com/tuna/tapa/tree/main/tests/regression/spmv-hisparse-mmap) | Sparse SpMV | HiSparse mmap-based data path | [FPGA'22](https://www.csl.cornell.edu/~zhiruz/pdfs/spmv-fpga2022.pdf) |
| [hbm-bandwidth](https://github.com/tuna/tapa/tree/main/tests/regression/hbm-bandwidth) | HBM bandwidth | `async_mmap` over all 32 HBM channels | Repository-only |
| [hbm-bandwidth-1-ch](https://github.com/tuna/tapa/tree/main/tests/regression/hbm-bandwidth-1-ch) | HBM bandwidth | Minimal single-channel `async_mmap` baseline | Repository-only |
| [stencil-dilate](https://github.com/tuna/tapa/tree/main/tests/regression/stencil-dilate) | Image dilation | 15 parallel 512-bit, 13-point stencil pipelines | Repository-only |

### Repository-only benchmarks

- **hbm-bandwidth** drives independent asynchronous readers and writers on all
  32 HBM channels to stress aggregate memory bandwidth.
- **hbm-bandwidth-1-ch** is the corresponding one-channel baseline, useful for
  isolating the `async_mmap` protocol without the full replicated design.
- **stencil-dilate** partitions a fixed 4096 × 4096 grid over 15 memory-channel
  pipelines and computes a 13-point maximum (morphological dilation) stencil.

```admonish note
These are large designs: they need Vitis HLS and are meant for evaluating
frequency and resource usage, not for learning the API. Start with
`tests/apps/` for that. Designs with TAPA hosts expose manual `-xosim` targets.
KNN fast cosimulation uses one tile per processing element while its canonical
XO retains 64; the full-size stencil workload can run for hours under xsim, so
use an appropriate `--test_timeout` override. New designs are added over time —
check `tests/regression/` in the repository for the current list.
```

---

**Next step:** [Common Errors](../troubleshoot/common-errors.md)
