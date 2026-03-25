<!--
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
-->

# TAPA

[![Staging Build](https://github.com/tuna/tapa/actions/workflows/staging-build.yml/badge.svg)](https://github.com/tuna/tapa/actions/workflows/staging-build.yml)
[![Documentation](https://readthedocs.org/projects/tapa/badge/?version=latest)](https://tapa.readthedocs.io/en/latest/?badge=latest)

TAPA is a powerful framework for designing high-frequency FPGA
dataflow accelerators. It provides a **powerful C++ API** for expressing
task-parallel designs with **advanced optimization techniques** to deliver
exceptional design performance and productivity.

TAPA is community maintained by
[Tsinghua University TUNA Association](https://tuna.moe/).

- **High-Frequency Performance**: Achieve 2x higher frequency on average
  compared to Vivado[<sup>1</sup>](https://doi.org/10.1145/3431920.3439289).
- **Rapid Development**: 7x faster compilation and 3x faster software
  simulation than Vitis HLS[<sup>2</sup>](https://doi.org/10.1109/fccm51124.2021.00032).
- **Expressive API**: Rich C++ syntax with dedicated APIs for complex memory
  access patterns and explicit parallelism.
- **HBM Optimizations**: Automated design space exploration and physical
  optimizations for HBM FPGAs.


## Quick Start

### Installing from Releases

The easiest way to install TAPA is from a pre-built release:

```sh
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
```

This downloads and installs the latest release. With root privileges, TAPA
installs to `/opt/tapa` with symlinks in `/usr/local/bin`. Otherwise it
installs to `~/.tapa` and adds it to your `PATH` via your shell profile.

To install a specific version:

```sh
TAPA_VERSION=0.1.20260319 \
  curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
```

Releases are available at
[github.com/tuna/tapa/releases](https://github.com/tuna/tapa/releases).

### Prerequisites

- Ubuntu 18.04+, Debian 10+, RHEL 9+, Fedora 34+, or Amazon Linux 2023
- Vitis HLS 2022.1 or later

### Building from Source

```bash
# Install dependencies (Ubuntu/Debian example)
sudo apt-get install g++ binutils git python3

# Install Bazel (see https://bazel.build/install)

# Clone the repository
git clone https://github.com/tuna/tapa.git
cd tapa

# Build TAPA
bazel build //...

# Build without tests
bazel build //... -- -//tests/...
```

See the [Building from Source](https://tapa.readthedocs.io/en/main/dev/build.html)
guide for detailed instructions.

### Compilation

```bash
cd tests/apps/vadd

# Software simulation (use bazel-bin/tapa/tapa if built from source)
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd
./vadd

# Hardware compilation and emulation
tapa compile \
   --top VecAdd \
   --part-num xcu250-figd2104-2L-e \
   --clock-period 3.33 \
   -f vadd.cpp \
   -o vecadd.xo
./vadd --bitstream=vecadd.xo 1000
```

### Visualization

TAPA includes a web-based visualizer in the ``tapa-visualizer/`` directory.
You can build and run it locally to visualize your design's ``graph.json``
file generated during compilation.

For detailed instructions, see our [User Guide](https://tapa.readthedocs.io/en/main/).


## Documentation

- [Quick Reference](https://tapa.readthedocs.io/en/main/user/cheatsheet.html).
- [User Guide](https://tapa.readthedocs.io/en/main/).
- [Installation Guide](https://tapa.readthedocs.io/en/main/user/installation.html).
- [Getting Started](https://tapa.readthedocs.io/en/main/user/getting_started.html).
- [API Reference](https://tapa.readthedocs.io/en/main/ref/api.html).


## Success Stories

- [Serpens](https://dl.acm.org/doi/10.1145/3489517.3530420) (DAC'22): 270 MHz
  on Xilinx Alveo U280 HBM board with 24 HBM channels, while the Vitis HLS
  baseline failed in routing.
- [Sextans](https://dl.acm.org/doi/pdf/10.1145/3490422.3502357) (FPGA'22):
  260 MHz on Xilinx Alveo U250 board with 4 DDR channels, while the Vivado
  baseline achieves only 189 MHz.
- [SPLAG](https://github.com/UCLA-VAST/splag) (FPGA'22): Up to 4.9x speedup
  over state-of-the-art FPGA accelerators, up to 2.6x speedup over 32-thread
  CPU running at 4.4 GHz, and up to 0.9x speedup over an A100 GPU.
- [AutoSA Systolic-Array Compiler](https://github.com/UCLA-VAST/AutoSA)
  (FPGA'21): Significant frequency improvements over the Vitis HLS baseline.
- [KNN](https://github.com/SFU-HiAccel/CHIP-KNN) (FPT'20): 252 MHz on Xilinx
  Alveo U280 board, compared to 165 MHz with the Vivado baseline.


## Licensing

TAPA is open-source software licensed under the MIT license.
For full license details, please refer to the
[LICENSE](https://github.com/tuna/tapa/blob/main/LICENSE) file.


## Publications

1. Licheng Guo, Yuze Chi, Jie Wang, Jason Lau, Weikang Qiao, Ecenur Ustun, Zhiru Zhang, Jason Cong.
   [AutoBridge: Coupling coarse-grained floorplanning and pipelining for high-frequency HLS design on multi-die FPGAs](https://doi.org/10.1145/3431920.3439289).
   FPGA, 2021. (Best Paper Award)
2. Yuze Chi, Licheng Guo, Jason Lau, Young-kyu Choi, Jie Wang, Jason Cong.
   [Extending high-level synthesis for task-Parallel programs](https://doi.org/10.1109/fccm51124.2021.00032).
   FCCM, 2021.
3. Young-kyu Choi, Yuze Chi, Jason Lau, Jason Cong.
   [TARO: Automatic optimization for free-running kernels in FPGA high-level synthesis](https://doi.org/10.1109/TCAD.2022.3216544).
   TCAD, 2022.
4. Licheng Guo, Pongstorn Maidee, Yun Zhou, Chris Lavin, Eddie Hung, Wuxi Li, Jason Lau, Weikang Qiao, Yuze Chi, Linghao Song, Yuanlong Xiao, Alireza Kaviani, Zhiru Zhang, Jason Cong.
   [RapidStream 2.0: Automated parallel implementation of latency insensitive FPGA designs through partial reconfiguration](https://doi.org/10.1145/3593025).
   TRETS, 2023.
5. Licheng Guo, Yuze Chi, Jason Lau, Linghao Song, Xingyu Tian, Moazin Khatti, Weikang Qiao, Jie Wang, Ecenur Ustun, Zhenman Fang, Zhiru Zhang, Jason Cong.
   [TAPA: A scalable task-parallel dataflow programming framework for modern FPGAs with co-optimization of HLS and physical design](https://doi.org/10.1145/3609335).
   TRETS, 2023.


---

Copyright (c) 2026 TAPA community maintainers and contributors.<br/>
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.<br/>
Copyright (c) 2020 Yuze Chi and contributors.<br/>
