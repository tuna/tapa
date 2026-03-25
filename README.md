<!--
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
-->

# TAPA

[![Staging Build](https://github.com/tuna/tapa/actions/workflows/staging-build.yml/badge.svg)](https://github.com/tuna/tapa/actions/workflows/staging-build.yml)
[![Documentation](https://readthedocs.org/projects/tapa/badge/?version=latest)](https://tapa.readthedocs.io/en/latest/?badge=latest)

TAPA is a task-parallel HLS framework that compiles C++ dataflow programs to Verilog RTL for Xilinx FPGAs. Software simulation runs on any Linux machine without FPGA hardware.

*C++ source → `tapa compile` → RTL (.xo) → Vitis v++ → FPGA bitstream*

TAPA is community maintained by the
[Tsinghua University TUNA Association](https://tuna.moe/).

Published results: 2× higher frequency on average versus Vivado
[[1]](https://doi.org/10.1145/3431920.3439289), with 7× faster compilation and
3× faster software simulation versus Vitis HLS
[[2]](https://doi.org/10.1109/fccm51124.2021.00032).


## Quick Start

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
```

With root privileges, installs to `/opt/tapa` (symlinks in `/usr/local/bin`).
Without root, installs to `~/.tapa` and updates your shell `PATH`.

**Requirements:** Linux (Ubuntu 18.04+, Debian 10+, RHEL 9+, Fedora 34+, Amazon
Linux 2023), `g++` 7.5.0+. Vitis HLS 2022.1+ is required for RTL synthesis and
on-board execution — **not** for software simulation.

To install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh \
  | TAPA_VERSION=0.1.20260319 sh -s -- -q
```

Releases: [github.com/tuna/tapa/releases](https://github.com/tuna/tapa/releases)

### Software simulation (no FPGA required)

```bash
# Compile kernel + host together using the tapa g++ wrapper
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd

# Run — executes on the CPU using TAPA's coroutine simulator
./vadd
```

Expected output:
```
I20000101 00:00:00.000000 0000000 task.h:66] running software simulation with TAPA library
kernel time: 1.19429 s
PASS!
```

### Compile to hardware

```bash
tapa compile \
  --top VecAdd \
  --part-num xcu280-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vadd.xo

# Run fast RTL cosimulation against the XO artifact
./vadd --bitstream=vadd.xo 1000
```


## Programming Model

A TAPA design is a directed graph of concurrent tasks connected by typed
streams. An upper-level task declares streams and launches child tasks; leaf
tasks perform computation. The same C++ code runs in software simulation and
compiles to RTL — no pragma changes required.

```cpp
// Kernel file (vadd.cpp)
void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a"), b_q("b"), c_q("c");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)   // reads DRAM → stream
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)   // stream → stream
      .invoke(Stream2Mmap, c_q, c, n); // stream → DRAM
}

// Host file (vadd-host.cpp)
tapa::invoke(VecAdd, FLAGS_bitstream,
             tapa::read_only_mmap<const float>(a),
             tapa::read_only_mmap<const float>(b),
             tapa::write_only_mmap<float>(c), n);
```

Key conventions:
- **Streams** are passed by reference (`tapa::istream<T>&`, `tapa::ostream<T>&`)
- **mmap** is passed by value (`tapa::mmap<T>`)
- **Upper-level tasks** contain only stream declarations and `.invoke()` chains — no computation
- `tapa::invoke` dispatches to software simulation (empty path), fast cosim (`.xo`), or on-board execution (`.xclbin`) based on the bitstream argument


## Documentation

Full documentation: **[tapa.readthedocs.io](https://tapa.readthedocs.io/en/latest/)**

| Section | Description |
|---------|-------------|
| [Installation](https://tapa.readthedocs.io/en/latest/start/installation.html) | Install from release or build from source |
| [Your First Run](https://tapa.readthedocs.io/en/latest/start/first-run.html) | Software simulation without FPGA hardware |
| [How-To Guides](https://tapa.readthedocs.io/en/latest/howto/software-simulation.html) | Build, simulate, and deploy designs |
| [Tutorials](https://tapa.readthedocs.io/en/latest/tutorials/learning-path.html) | Annotated labs from vadd to floorplanning |
| [C++ API Reference](https://tapa.readthedocs.io/en/latest/reference/api.html) | Full API: tasks, streams, mmap, utilities |
| [CLI Reference](https://tapa.readthedocs.io/en/latest/reference/cli.html) | All `tapa` subcommands and flags |
| [Troubleshooting](https://tapa.readthedocs.io/en/latest/troubleshoot/common-errors.html) | Common errors, deadlocks, cosim issues |


## Building from Source

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt-get install g++ binutils git python3

# Install Bazel — see https://bazel.build/install

git clone https://github.com/tuna/tapa.git
cd tapa
bazel build //...
```

See [Building from Source](https://tapa.readthedocs.io/en/latest/developer/build.html) for the full guide.


## Published Results

- [Serpens](https://dl.acm.org/doi/10.1145/3489517.3530420) (DAC'22): 270 MHz
  on Xilinx Alveo U280 with 24 HBM channels; the Vitis HLS baseline failed to route.
- [Sextans](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357) (FPGA'22):
  260 MHz on Xilinx Alveo U250 versus 189 MHz with Vivado baseline.
- [SPLAG](https://github.com/UCLA-VAST/splag) (FPGA'22): Up to 4.9× speedup
  over prior FPGA accelerators; up to 0.9× vs. A100 GPU.
- [AutoSA](https://github.com/UCLA-VAST/AutoSA) (FPGA'21): Systolic-array compiler
  with frequency improvements over Vitis HLS baseline.
- [Callipepla](https://github.com/UCLA-VAST/Callipepla) (FPGA'23): 3.94× speedup
  over Xilinx XcgSolver; 3.34× better energy efficiency than A100 GPU.
- [LevelST](https://dl.acm.org/doi/10.1145/3626202.3637568) (FPGA'24): 2.65×
  speedup, 9.82× higher energy efficiency vs. V100/RTX 3060 with cuSPARSE.
- [CHIP-KNN](https://github.com/SFU-HiAccel/CHIP-KNN) (ICFPT'20 / TRETS'23):
  252 MHz on Alveo U280 versus 165 MHz with Vivado; v2 up to 45× over 48-thread CPU.


## Publications

Core papers describing the TAPA compiler and the physical design toolflow it integrates:

1. Yuze Chi et al. [Extending high-level synthesis for task-parallel programs](https://doi.org/10.1109/fccm51124.2021.00032). FCCM, 2021.
2. Licheng Guo et al. [TAPA: A scalable task-parallel dataflow programming framework for modern FPGAs with co-optimization of HLS and physical design](https://doi.org/10.1145/3609335). TRETS, 2023.
3. Licheng Guo et al. [AutoBridge: Coupling coarse-grained floorplanning and pipelining for high-frequency HLS design on multi-die FPGAs](https://doi.org/10.1145/3431920.3439289). FPGA, 2021. **(Best Paper Award)**
4. Young-kyu Choi et al. [TARO: Automatic optimization for free-running kernels in FPGA high-level synthesis](https://doi.org/10.1109/TCAD.2022.3216544). TCAD, 2022.
5. Licheng Guo et al. [RapidStream: Parallel physical implementation of FPGA HLS designs](https://dl.acm.org/doi/10.1145/3490422.3502361). FPGA, 2022. **(Best Paper Award)**
6. Licheng Guo et al. [RapidStream 2.0: Automated parallel implementation of latency-insensitive FPGA designs through partial reconfiguration](https://doi.org/10.1145/3593025). TRETS, 2023.
7. Jason Lau et al. [RapidStream IR: Infrastructure for FPGA high-level physical synthesis](https://vast.cs.ucla.edu/sites/default/files/publications/522_Final_Manuscript.pdf). ICCAD, 2024.
8. Neha Prakriya et al. [TAPA-CS: Enabling scalable accelerator design on distributed HBM-FPGAs](https://arxiv.org/abs/2311.10189). ASPLOS, 2024.
9. Moazin Khatti et al. [PASTA: Programming and automation support for scalable task-parallel HLS programs on modern multi-die FPGAs](https://about.blaok.me/publication/fccm23-pasta/). FCCM, 2023 / TRETS, 2024.
10. Suhail Basalama, Jason Cong. [Stream-HLS: Towards automatic dataflow acceleration](https://dl.acm.org/doi/10.1145/3706628.3708878). FPGA, 2025.
11. Akhil Raj Baranwal, Zhenman Fang. [PoCo: Extending task-parallel HLS programming with shared multi-producer multi-consumer buffer support](https://www.sfu.ca/~zhenman/files/J22-TRETS2025-PoCo.pdf). TRETS, 2025.

For annotated descriptions and the full list of application papers, see [Publications](https://tapa.readthedocs.io/en/latest/reference/publications.html).


## License

TAPA is open-source software licensed under the MIT license.
See [LICENSE](https://github.com/tuna/tapa/blob/main/LICENSE) for details.


---

Copyright (c) 2026 TAPA community maintainers and contributors.<br/>
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.<br/>
Copyright (c) 2020 Yuze Chi and contributors.<br/>
