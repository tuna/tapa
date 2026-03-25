# Examples Catalog

The TAPA repository includes two sets of example designs. Small self-contained examples live under `tests/apps/`. Larger benchmarks live under `tests/regression/`.

---

## Small examples

| Example | Problem type | Key TAPA feature | Location |
|---------|-------------|-----------------|----------|
| vadd | Vector addition | Basic streams + mmap | `tests/apps/vadd` |
| bandwidth | Memory bandwidth benchmark | `async_mmap`, 32 HBM channels | `tests/apps/bandwidth` |
| network | Packet switching | `peek`, detached tasks, hierarchical tasks | `tests/apps/network` |
| cannon | Cannon's matrix multiply | 2D stream arrays, systolic | `tests/apps/cannon` |
| jacobi | Stencil computation | End-of-transmission (`close()`) | `tests/apps/jacobi` |

---

## Published benchmarks

| Example | Problem type | Key feature | Published in |
|---------|-------------|-------------|-------------|
| autosa mm/10x13 | Matrix multiplication | AutoSA-generated systolic (90% U55C LUT) | — |
| callipepla | Conjugate gradient | 26 HBM channels | [FPGA'23](https://dl.acm.org/doi/pdf/10.1145/3543622.3573182) |
| cnn | CNN systolic array | Multi-SLR | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| lu_decompose | LU systolic array | Multi-SLR | [FPGA'21](https://dl.acm.org/doi/pdf/10.1145/3431920.3439292) |
| hbm-bandwidth | HBM bandwidth profiler | `async_mmap`, all 32 channels | — |
| hbm-bandwidth-1-ch | HBM bandwidth (1 channel) | Minimal `async_mmap` | — |
| serpens | Sparse SpMV | Multiple HBM channels, scalable parallelism | [DAC'22](https://arxiv.org/pdf/2111.12555.pdf) |
| spmm | Sparse SpMM | HBM streams | [FPGA'22](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357) |
| spmv-hisparse-mmap | Sparse SpMV (HiSparse) | mmap-based SpMV | [FPGA'22](https://www.csl.cornell.edu/~zhiruz/pdfs/spmv-fpga2022.pdf) |
| knn | K-nearest-neighbor | FPT accelerator | [FPT'20](http://www.sfu.ca/~zhenman/files/C19-FPT2020-CHIP-KNN.pdf) |
| page_rank | Page Rank | FCCM accelerator | [FCCM'21](https://about.blaok.me/pub/fccm21-tapa.pdf) |

```admonish note
The `tests/regression/` directory is under active development; new designs are added regularly. Check the repository for the latest list.
```

---

**Next step:** [Common Errors](../troubleshoot/common-errors.md)
