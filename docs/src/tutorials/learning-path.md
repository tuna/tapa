# Learning Path

These labs walk through the TAPA programming model from first principles to advanced topics. Each lab builds on the previous one — you will understand each concept more deeply if you complete them in order. Allow roughly three hours to work through all five labs.

## Labs

| Lab | Topic | Prerequisites | Time | Skip if... |
|-----|-------|--------------|------|-----------|
| [Lab 1: Vector Add](lab-01-vadd.md) | Core programming model | [Your First Run](../start/first-run.md) | 20 min | You already understand task graphs and mmap |
| [Lab 2: High-Bandwidth Memory](lab-02-async-mmap.md) | async\_mmap for memory throughput | Lab 1 | 30 min | You only need basic mmap |
| [Lab 3: Migrating from Vitis HLS](lab-03-vitis-hls.md) | Porting existing HLS code | Lab 1 | 30 min | You are new to FPGA HLS |
| [Lab 4: Custom RTL Modules](lab-04-custom-rtl.md) | Integrating hand-written RTL | Lab 1 | 45 min | You don't need to integrate RTL |
| [Lab 5: Floorplan & DSE](lab-05-floorplan.md) | Floorplanning for multi-SLR FPGAs | Lab 2 | 60 min | You are not targeting multi-SLR devices |

## Where to start

**New to FPGA HLS** — Start at Lab 1. It introduces the task graph model that every later lab assumes you understand.

**Coming from Vitis HLS** — Lab 3 covers the mechanical differences, but reading Lab 1 first is worthwhile because TAPA's concurrency model is structurally different from standard HLS. If you have already read the [Programming Model](../concepts/programming-model.md) page, you can go directly to Lab 3.

**Already ran vadd in First Run** — You have seen the commands; Lab 1 does the deep-dive explanation of why the code is structured the way it is. It is worth reading even if the output was correct.

**Need HBM throughput** — Work through Lab 2 (async\_mmap) and then Lab 5 (floorplanning). Both are required to get full memory bandwidth on multi-SLR devices.

## Background reading

Before starting any lab, the [Programming Model](../concepts/programming-model.md) page covers the vocabulary used throughout: task graphs, streams, mmap, and the compile pipeline. The labs assume you have read at least the [Programming Model](../concepts/programming-model.md) page.

**Start here:** [Lab 1: Vector Add](lab-01-vadd.md)
