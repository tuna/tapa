# Publications

Papers describing the TAPA compiler, the physical design toolflow it integrates, and accelerators built with TAPA.

---

## Core Publications

### TAPA Compiler

**Yuze Chi, Licheng Guo, Jason Lau, Young-kyu Choi, Jie Wang, Jason Cong.**
Extending High-Level Synthesis for Task-Parallel Programs.
*IEEE FCCM*, 2021.
[[PDF](https://arxiv.org/pdf/2009.11389.pdf)] [[Code](https://github.com/UCLA-VAST/tapa)]

Introduces the TAPA task API, coroutine-based software simulation (3.2× faster than Vitis HLS sequential simulation), and fast hierarchical RTL generation (6.8× faster QoR iteration). Reduces kernel and host code by 22% and 51% on average versus Vitis HLS dataflow.

---

**Licheng Guo, Yuze Chi, Jason Lau, Linghao Song, Xingyu Tian, Moazin Khatti, Weikang Qiao, Jie Wang, Ecenur Ustun, Zhenman Fang, Zhiru Zhang, Jason Cong.**
TAPA: A Scalable Task-Parallel Dataflow Programming Framework for Modern FPGAs with Co-optimization of HLS and Physical Design.
*ACM TRETS*, 2023.
[[PDF](https://www.sfu.ca/~zhenman/files/J14-TRETS2023-TAPA.pdf)] [[Code](https://github.com/UCLA-VAST/tapa)]

Full journal treatment of the TAPA compiler and runtime. Average frequency improves from 147 MHz to 297 MHz (102%) across 43 designs; 16 previously unroutable designs achieve 274 MHz on average after co-optimization with physical design.

---

### Floorplanning and Physical Design

**Licheng Guo, Yuze Chi, Jie Wang, Jason Lau, Weikang Qiao, Ecenur Ustun, Zhiru Zhang, Jason Cong.**
AutoBridge: Coupling Coarse-Grained Floorplanning and Pipelining for High-Frequency HLS Design on Multi-Die FPGAs.
*ACM/SIGDA FPGA*, 2021. **(Best Paper Award)**
[[PDF](https://dl.acm.org/doi/pdf/10.1145/3431920.3439289)] [[Code](https://github.com/UCLA-VAST/AutoBridge)]

Doubles achievable clock frequency on average by automatically floorplanning HLS dataflow designs across SLR boundaries and inserting pipeline registers. Now maintained exclusively as a plug-in of the TAPA workflow.

---

**Licheng Guo, Pongstorn Maidee, Yun Zhou, Chris Lavin, Jie Wang, Yuze Chi, Weikang Qiao, Alireza Kaviani, Zhiru Zhang, Jason Cong.**
RapidStream: Parallel Physical Implementation of FPGA HLS Designs.
*ACM/SIGDA FPGA*, 2022. **(Best Paper Award)**
[[PDF](https://dl.acm.org/doi/pdf/10.1145/3490422.3502361)]

Split compilation with parallel placement and routing per partition. Achieves 5–7× compile time reduction and up to 1.3× frequency increase on Xilinx U250.

---

**Licheng Guo, Pongstorn Maidee, Yun Zhou, Chris Lavin, Eddie Hung, Wuxi Li, Jason Lau, Weikang Qiao, Yuze Chi, Linghao Song, Yuanlong Xiao, Alireza Kaviani, Zhiru Zhang, Jason Cong.**
RapidStream 2.0: Automated Parallel Implementation of Latency-Insensitive FPGA Designs through Partial Reconfiguration.
*ACM TRETS*, 2023.
[[Link](https://dl.acm.org/doi/10.1145/3593025)]

Extends RapidStream with virtual pins and partial reconfiguration. Achieves 5–7× compile time reduction and 1.3× frequency increase on Xilinx U280, approximately 2× faster than RapidStream 1.0.

---

**Jason Lau, Yuanlong Xiao, Yutong Xie, Yuze Chi, Linghao Song, Shaojie Xiang, Michael Lo, Zhiru Zhang, Jason Cong, Licheng Guo.**
RapidStream IR: Infrastructure for FPGA High-Level Physical Synthesis.
*IEEE/ACM ICCAD*, 2024.
[[PDF](https://vast.cs.ucla.edu/sites/default/files/publications/522_Final_Manuscript.pdf)]

Generalizes RapidStream into a reusable IR for FPGA high-level physical synthesis. Supports multiple task-parallel HLS frontends including TAPA and PASTA.

---

### Compiler Extensions

**Young-kyu Choi, Yuze Chi, Jason Lau, Jason Cong.**
TARO: Automatic Optimization for Free-Running Kernels in FPGA High-Level Synthesis.
*IEEE TCAD*, 2022.
[[Link](https://ieeexplore.ieee.org/document/9926183/)]

Eliminates unnecessary control logic for streaming applications. Achieves 16% LUT and 45% FF reduction on systolic-array designs on Alveo U250. Integrated into the TAPA compilation flow.

---

**Neha Prakriya, Yuze Chi, Suhail Basalama, Linghao Song, Jason Cong.**
TAPA-CS: Enabling Scalable Accelerator Design on Distributed HBM-FPGAs.
*ACM ASPLOS*, 2024.
[[arXiv](https://arxiv.org/abs/2311.10189)] [[Code](https://github.com/UCLA-VAST/TAPA-CS)]

Extends TAPA to automatically partition designs across a cluster of FPGAs with the `--multi-fpga N` compiler flag. Handles congestion control, resource balancing, and inter-FPGA pipelining.

---

**Moazin Khatti, Xingyu Tian, Yuze Chi, Licheng Guo, Jason Cong, Zhenman Fang.**
PASTA: Programming and Automation Support for Scalable Task-Parallel HLS Programs on Modern Multi-Die FPGAs.
*IEEE FCCM*, 2023; extended in *ACM TRETS*, 2024.
[[Link](https://about.blaok.me/publication/fccm23-pasta/)]

Adds automated latency-insensitive buffer (ping-pong) channel synthesis alongside FIFO streams in the task-parallel HLS flow, targeting the same class of multi-die FPGA designs as TAPA.

---

**Suhail Basalama, Jason Cong.**
Stream-HLS: Towards Automatic Dataflow Acceleration.
*ACM/SIGDA FPGA*, 2025.
[[Paper](https://dl.acm.org/doi/10.1145/3706628.3708878)] [[Code](https://github.com/UCLA-VAST/Stream-HLS)]

MLIR-based compiler that takes PyTorch or C/C++ and automatically generates optimized TAPA dataflow accelerators. Outperforms prior automation frameworks by up to 79× and manually-optimized TAPA designs by up to 11× geometric mean.

---

**Akhil Raj Baranwal, Zhenman Fang.**
PoCo: Extending Task-Parallel HLS Programming with Shared Multi-Producer Multi-Consumer Buffer Support.
*ACM TRETS*, 2025.
[[PDF](https://www.sfu.ca/~zhenman/files/J22-TRETS2025-PoCo.pdf)]

Generalizes TAPA and PASTA's point-to-point SPSC channels to shared multi-producer–multi-consumer buffer abstractions with placement-aware optimizations for multi-die FPGAs.

---

## Application Papers

Accelerators built with the TAPA compiler and toolflow.

### Sparse Linear Algebra

**Linghao Song, Yuze Chi, Atefeh Sohrabizadeh, Young-kyu Choi, Jason Lau, Jason Cong.**
Sextans: A Streaming Accelerator for General-Purpose Sparse-Matrix Dense-Matrix Multiplication.
*ACM/SIGDA FPGA*, 2022.
[[PDF](https://par.nsf.gov/servlets/purl/10350115)] [[Code](https://github.com/linghaosong/Sextans/tree/tapa)]

SpMM accelerator on Alveo U280/U250. TAPA/AutoBridge-compiled DDR variant achieves 260 MHz versus a Vivado baseline of 189 MHz. Up to 2.50× geomean speedup over NVIDIA K80.

---

**Linghao Song, Yuze Chi, Licheng Guo, Jason Cong.**
Serpens: A High Bandwidth Memory Based Accelerator for General-Purpose Sparse Matrix-Vector Multiplication.
*ACM/IEEE DAC*, 2022.
[[Code](https://github.com/linghaosong/Serpens)]

SpMV accelerator on Alveo U280 using 24 HBM channels. The Vitis HLS baseline failed to route; TAPA + AutoBridge achieves 270 MHz and up to 60.55 GFLOP/s.

---

**Linghao Song, Licheng Guo, Suhail Basalama, Yuze Chi, Robert F. Lucas, Jason Cong.**
Callipepla: Stream Centric Instruction Set and Mixed Precision for Accelerating Conjugate Gradient Solver.
*ACM/SIGDA FPGA*, 2023.
[[Code](https://github.com/UCLA-VAST/Callipepla)]

Conjugate gradient solver on U280 HBM. 3.94× speedup and 2.94× better energy efficiency over Xilinx XcgSolver; 3.34× better energy efficiency and 77% throughput of an A100 GPU at 4× lower memory bandwidth. Built with TAPA and AutoBridge.

---

**Zifan He, Linghao Song, Robert F. Lucas, Jason Cong.**
LevelST: Stream-based Accelerator for Sparse Triangular Solver.
*ACM/SIGDA FPGA*, 2024.
[[Paper](https://dl.acm.org/doi/10.1145/3626202.3637568)] [[Code](https://github.com/OswaldHe/LevelST)]

First HBM-FPGA accelerator for SpTRSV. 2.65× speedup and 9.82× higher energy efficiency versus V100/RTX 3060 with cuSPARSE. Built on TAPA with AutoBridge floorplanning.

---

**Manoj B. Rajashekar, Xingyu Tian, Zhenman Fang.**
HiSpMV / MAD-HiSpMV: Hybrid Row Distribution and Vector Buffering for Imbalanced SpMV Acceleration on FPGAs.
*ACM/SIGDA FPGA*, 2024; extended in *ACM TRETS*, 2025.
[[Paper](https://dl.acm.org/doi/10.1145/3772082)] [[Code](https://github.com/SFU-HiAccel/HiSpMV)]

SpMV accelerator on Alveo U280 adapting row distribution to matrix structure. Uses TAPA for hardware build, cosimulation, and hardware emulation.

---

**Ahmad Sedigh Baroughi, Xingyu Tian, Moazin Khatti, Akhil Raj Baranwal, Yuze Chi, Licheng Guo, Jason Cong, Zhenman Fang.**
HiSpMM: High Performance High Bandwidth Sparse-Dense Matrix Multiplication on HBM-equipped FPGAs.
*ACM TRETS*, 2025.
[[Paper](https://dl.acm.org/doi/10.1145/3774327)] [[Code](https://github.com/SFU-HiAccel/HiSpMM)]

SpMM accelerator on Alveo U280 using TAPA for hardware generation, cosimulation, and runtime.

---

### Graph Analytics

**Yuze Chi, Licheng Guo, Jason Cong.**
Accelerating SSSP for Power-Law Graphs (SPLAG).
*ACM/SIGDA FPGA*, 2022.
[[Paper](https://about.blaok.me/publication/splag/)] [[Code](https://github.com/UCLA-VAST/SPLAG)]

FPGA SSSP accelerator on Alveo U280. Up to 4.9× over prior FPGA accelerators, 2.6× over a 32-thread CPU, 0.9× of A100 GPU at 4.1× the power budget. Fully parameterized TAPA HLS C++ implementation.

---

### Systolic Arrays and Machine Learning

**Jie Wang, Licheng Guo, Jason Cong.**
AutoSA: A Polyhedral Compiler for High-Performance Systolic Arrays on FPGA.
*ACM/SIGDA FPGA*, 2021.
[[Paper](https://dl.acm.org/doi/10.1145/3431920.3439292)] [[Code](https://github.com/UCLA-VAST/AutoSA)]

Polyhedral systolic array compiler targeting MM, CNN, LU, MTTKRP. Integrated with TAPA and AutoBridge for routing congestion resolution and frequency improvement.

---

**Suhail Basalama, Atefeh Sohrabizadeh, Jie Wang, Licheng Guo, Jason Cong.**
FlexCNN: An End-to-End Framework for Composing CNN Accelerators on FPGA.
*ACM TRETS*, 2023.
[[Paper](https://dl.acm.org/doi/10.1145/3570928)] [[Code](https://github.com/UCLA-VAST/FlexCNN)]

CNN compilation framework for OpenPose, U-Net, E-Net, and VGG-16 on Alveo U250/U280. TAPA code generation added as a journal contribution. 2.3× performance improvement; 5× further speedup via software-hardware pipelining.

---

### K-Nearest Neighbors

**Alec Lu, Zhenman Fang, Nazanin Farahpour, Lesley Shannon.**
CHIP-KNN: A Configurable and High-Performance K-Nearest Neighbors Accelerator on Cloud FPGAs.
*IEEE ICFPT*, 2020.
[[Code](https://github.com/SFU-HiAccel/CHIP-KNN)]

KNN accelerator on Alveo U280. TAPA-compiled design achieves 252 MHz versus a Vivado baseline of 165 MHz.

---

**Kenneth Liu, Alec Lu, Kartik Samtani, Zhenman Fang, Licheng Guo.**
CHIP-KNNv2: A Configurable and High-Performance K-Nearest Neighbors Accelerator on HBM-based FPGAs.
*ACM TRETS*, 2023.
[[Paper](https://dl.acm.org/doi/10.1145/3616873)] [[Code](https://github.com/SFU-HiAccel/CHIP-KNN)]

Streaming-based redesign on Alveo U280 with automated TAPA HLS C code generation. Up to 45× speedup over a 48-thread CPU.

---

### Multi-FPGA Applications

**Tianqi Zhang, Neha Prakriya, Sumukh Pinge, Jason Cong, Tajana Rosing.**
SpectraFlux: Harnessing the Flow of Multi-FPGA in Mass Spectrometry Clustering.
*ACM/IEEE DAC*, 2024.
[[Paper](https://dl.acm.org/doi/10.1145/3649329.3657354)]

Uses TAPA-CS to partition a mass spectrometry clustering workload across multiple networked HBM-FPGAs.
