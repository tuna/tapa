# Glossary

---

**analyze**

The `tapa analyze` step. Parses the C++ source with `tapacc` (a Clang-based tool) and extracts the task graph and inter-task channels to `graph.json` in the work directory. This step does not invoke any vendor tools and runs on any host.

---

**async_mmap**

A decoupled memory access type (`tapa::async_mmap<T>`). Instead of stalling on each memory operation, the kernel issues requests through address FIFOs and collects results through data and response FIFOs independently. This decoupling allows the kernel to keep the memory bus busy even when computation is not complete, enabling higher effective memory bandwidth. `async_mmap` must be passed by reference in task signatures.

---

**backpressure**

The condition where a producer cannot write to a stream because the downstream consumer has not yet drained elements from the FIFO and the buffer is full. The producer blocks until the consumer reads at least one element. Backpressure propagates naturally through TAPA streams and is the primary flow-control mechanism.

---

**cosim** (see also: *fast cosim*)

Hardware cosimulation. Runs RTL simulation using the XO artifact to verify the hardware implementation against the software model. TAPA supports fast cosim, which uses the XO directly without running full Vivado implementation. See also: *fast cosim*.

---

**detached task**

A task invoked with `.invoke<tapa::detach>()`. A detached task runs concurrently with its siblings but the parent does not wait for it to finish before returning. Useful for background tasks such as monitors or credit managers. See `tapa::task` in the [API reference](api.md).

---

**EoT** (end-of-transaction)

A sentinel value written to a stream to signal the end of a data sequence. The producer calls `ostream::close()` to write the EoT marker; the consumer calls `istream::open()` to consume it. The `TAPA_WHILE_NOT_EOT` macro automates looping until EoT is detected.

---

**fast cosim**

Synonym for *cosim* in the TAPA context. The `tapa cosim` command runs RTL simulation directly from the XO file without a full Vivado implementation run, making it significantly faster than traditional cosim flows.

---

**leaf task**

A task that contains only computation and does not call `.invoke()`. Leaf tasks are the units of synthesis: each leaf task is compiled to RTL by Vitis HLS independently. A leaf task may use streams, mmap, or async_mmap parameters.

---

**mmap**

Memory-mapped region. A contiguous block of host memory exposed to the kernel as a pointer-like handle (`tapa::mmap<T>`). The kernel accesses it synchronously, similar to a C pointer. For pipelined non-blocking access, use `async_mmap` instead. `mmap` is passed by value in task signatures.

---

**mmaps**

An array of N mmap regions (`tapa::mmaps<T, N>`) passed as a single argument. The framework distributes one region per child task invocation when the parent iterates over N instances.

---

**pack**

The `tapa pack` step. Packages per-task RTL produced by `tapa synth` into a single XO (or ZIP) artifact suitable for passing to `v++` or for use in fast cosim.

---

**remote execution**

Offloading vendor-tool steps (HLS, pack) to a remote Linux host over SSH. Configured with `--remote-host`. The local machine runs `tapacc` (the analyze step) and transfers source files; the remote host runs Vitis HLS. Useful when cross-compiling from macOS or when the local machine lacks a Vitis licence.

---

**stream**

A FIFO channel between tasks (`tapa::stream<T, Depth>`). Streams are the fundamental communication primitive in TAPA. A stream is declared in an upper-level task and passed to child tasks as `istream<T>&` (read end) or `ostream<T>&` (write end). The FIFO enforces backpressure automatically.

---

**stream depth**

The number of elements the FIFO can hold before the producer blocks. Declared as the second template parameter of `tapa::stream<T, Depth>`. The default depth is 2. Increasing depth decouples producer and consumer and can improve throughput at the cost of FPGA BRAM or LUT resources.

---

**synth**

The `tapa synth` step. Runs Vitis HLS on each leaf task extracted during `tapa analyze` to produce per-task Verilog RTL. Results are stored in `tar/` and `hdl/` under the work directory.

---

**TAPA_CONCURRENCY**

Environment variable controlling the number of coroutine threads used during software simulation. Set to `1` to force sequential execution (useful for debugging). Set to a value greater than 1 to simulate parallel task execution. The default depends on the number of tasks in the design.

---

**top-level task** (upper-level task)

A task that only invokes other tasks via `tapa::task().invoke()` and contains no direct computation. A top-level task maps to a system-level wrapper in RTL that wires sub-task ports together. The top-level task is specified with `--top` on the `tapa` command line.

---

**work directory**

The directory where TAPA stores all intermediate artifacts between pipeline steps. Set with `--work-dir`. The default is `work.out/` in the current directory. See [Output Files](output-files.md) for the full directory structure.

---

**xclbin**

Xilinx compiled binary. The final bitstream file produced by Vivado implementation. An xclbin is loaded onto the FPGA by the host application at runtime (via XRT or FRT). It is produced by running `v++ --link` on an XO file.

---

**xo**

Xilinx object file. The intermediate artifact produced by `tapa pack`, containing all per-task RTL and metadata in a ZIP archive. The XO is the input to `v++ --link` for bitstream generation, and also the input to `tapa cosim` for fast hardware cosimulation.
