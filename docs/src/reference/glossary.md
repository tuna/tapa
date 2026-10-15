# Glossary

---

**analyze**

The `tapa analyze` step. Parses C++ with `tapacc` (a Clang-based tool) and
writes the task graph and inter-task channels to `tapa.json` in the work
directory. This step runs locally and does not invoke vendor tools.

---

**async_mmap**

A decoupled memory access type (`tapa::async_mmap<T>`). The kernel issues
requests through address FIFOs and collects data and responses through
independent FIFOs, rather than leaving all request scheduling to HLS. This can
keep multiple memory requests outstanding and hide latency. `async_mmap` must
be passed by reference in task signatures.

---

**backpressure**

The condition where a producer cannot write to a stream because the downstream consumer has not yet drained elements from the FIFO and the buffer is full. The producer blocks until the consumer reads at least one element. Backpressure propagates naturally through TAPA streams and is the primary flow-control mechanism.

---

**cosim** (see also: *fast cosim*)

RTL simulation used to check hardware behavior against the software model.
TAPA supports fast cosim from an XO or ZIP without Vitis implementation, and
Vitis hardware emulation from an `hw_emu.xclbin`. See also: *fast cosim*.

---

**DSE** (design-space exploration)

The `tapa floorplan --dse` mode. Instead of producing one floorplan, it sweeps a range of per-slot logic-utilization caps, implements each candidate with Vitis `v++ --link` or V80 SLASH, and keeps the one with the highest achieved frequency. Results land in `floorplan-metrics.json` (the winner) and `dse/candidates.json` (every candidate).

---

**detached task**

A task invoked with `.invoke<tapa::detach>()`. A detached task runs concurrently with its siblings but the parent does not wait for it to finish before returning. Useful for background tasks such as monitors or credit managers. See `tapa::task` in the [API reference](api.md).

---

**EoT** (end-of-transaction)

A sentinel value written to a stream to signal the end of a data sequence. The producer calls `ostream::close()` to write the EoT marker; the consumer calls `istream::open()` to consume it. The `TAPA_WHILE_NOT_EOT` macro automates looping until EoT is detected.

---

**fast cosim**

TAPA's lightweight RTL cosimulation flow, distinct from Vitis hardware
emulation. It is invoked by passing a `.xo` or `.zip` as the host executable's
`--bitstream` argument. The runtime launches xsim or Verilator without running
Vitis implementation.

---

**floorplan**

The optional `tapa floorplan` step, run between `synth` and `pack`. It places
tasks across SLRs, balances resource usage, and pipelines channels that cross
SLR boundaries to improve timing. It writes `floorplan.xdc`, which `pack`
applies automatically.

---

**leaf task**

A task that contains only computation and does not call `.invoke()`. Leaf tasks are the units of synthesis: each leaf task is compiled to RTL by Vitis HLS independently. A leaf task may use streams, mmap, or async_mmap parameters.

---

**mmap**

Memory-mapped region. A contiguous block of host memory exposed to the kernel
as a pointer-like handle (`tapa::mmap<T>`). HLS schedules its accesses; use
`async_mmap` when you need explicit request/response scheduling. `mmap` is
passed by value in task signatures.

---

**mmaps**

An array of N mmap regions (`tapa::mmaps<T, N>`) passed as a single argument. The framework distributes one region per child task invocation when the parent iterates over N instances.

---

**pack**

The `tapa pack` step. Packages RTL produced by `tapa synth` into one XO for
`v++ --link` and fast cosim, or one ZIP for direct RTL consumers and fast
cosim.

---

**remote execution**

Offloading vendor-tool steps to a remote Linux host over SSH, configured
with `--remote-host`. Analysis and planning-only floorplanning stay local;
synthesis, Vitis XO packaging, and implementation floorplanning can run
remotely. TAPA transfers each job's sources and outputs automatically.

---

**SLR** (super logic region)

One die of a multi-die FPGA. Large Alveo parts are built from two to four SLRs joined by a limited number of cross-die wires, and a signal crossing that boundary costs significantly more delay than one staying inside a die. This is what *floorplan* exists to manage.

---

**stream**

A FIFO channel between tasks (`tapa::stream<T, Depth>`). Streams are the fundamental communication primitive in TAPA. A stream is declared in an upper-level task and passed to child tasks as `istream<T>&` (read end) or `ostream<T>&` (write end). The FIFO enforces backpressure automatically.

---

**stream depth**

The number of elements the FIFO can hold before the producer blocks. Declared
as the second template parameter of `tapa::stream<T, Depth>`; the default is
2. Increasing depth lets a producer run further ahead and can improve
throughput at the cost of LUT, SRL, BRAM, or URAM resources.

---

**synth**

The `tapa synth` step. Runs Vitis HLS on each non-ignored task extracted during `tapa analyze`, then generates the connected Verilog hierarchy. Per-task HLS results are stored under `hls/`, and the complete RTL tree is stored under `rtl/` in the work directory.

---

**TAPA_CONCURRENCY**

Environment variable controlling the worker threads that run task coroutines.
Set to `1` to use one worker with cooperative interleaving at blocking points;
it does not run each task to completion. The default is the host's physical
CPU core count.

---

**top-level task**

The kernel entry point selected with `--top`. For `xilinx-vitis`, it must be
an upper-level task; `xilinx-hls` also permits a leaf top.

---

**upper-level task**

An orchestration task that declares channels and invokes children with
`tapa::task().invoke()` but performs no computation itself. In RTL, it wires
child interfaces and control logic together.

---

**work directory**

The directory where TAPA stores all intermediate artifacts between pipeline steps. Set with `--work-dir`. The default is `work.out/` in the current directory. See [Output Files](output-files.md) for the full directory structure.

---

**Verilator**

An open-source Verilog simulator, and one of the two backends for TAPA fast cosim. Select it with `-cosim_simulator verilator` on the host executable. It needs no Vivado installation and runs on Linux and macOS, but does not support Vivado waveform capture, and must be version 5.044 or newer — older releases cannot elaborate the AXI master Vitis HLS generates for an `mmap` port.

---

**xsim**

The Verilog simulator that ships with Vivado, and the default backend for TAPA fast cosim. Linux only, and requires a Vivado installation on the machine running the host executable. Unlike *Verilator*, it can save waveforms (`-xsim_save_waveform`) for inspection in the Vivado GUI.

---

**xclbin**

Xilinx container produced by `v++ --link` from an XO. An `hw.xclbin`
contains an on-board bitstream; an `hw_emu.xclbin` contains the hardware-
emulation model. The host loads either through XRT/FRT as appropriate.

---

**xo**

Xilinx object file. The intermediate artifact produced by `tapa pack`, containing all per-task RTL and metadata in a ZIP archive. The XO is the input to `v++ --link` for bitstream generation, and is also passed as `--bitstream` to the host executable for fast hardware cosimulation.
