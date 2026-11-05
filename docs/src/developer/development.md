# Developing TAPA

```admonish note
This section is intended for **developers** who want to contribute to TAPA.
It explains the development process, the code structure, and the guidelines
for contributing to the TAPA framework.
```

## Development Environment

TAPA enforces a consistent coding style and provides tools to ensure code
quality. Follow these steps to set up your development environment.

### Install Pre-Commit Hooks

```bash
pre-commit --version
pre-commit install
```

Install `pre-commit` with your OS package manager if the command is not
available. Pre-commit hooks run automatically before each commit to ensure code
compliance with style guidelines. To manually run the checks:

```bash
pre-commit run --all-files
```

## Code Structure

The TAPA codebase is organized into several key directories:

- `bazel/`: Contains Bazel build configurations.

  It defines how the TAPA compiler is used in the Bazel build system, and
  provides additional utilities for building and testing TAPA.

- `docs/`: Includes documentation files.

  The documentation is written in Markdown and built using mdBook.

- `fpga-runtime/`: Provides the FPGA runtime library.

  The runtime executes a design from the supplied artifact. It drives fast
  cosimulation from an XO or ZIP and uses XRT for Vitis hardware emulation or
  on-board execution from an XCLBIN.

- `tapa-lib/`: Houses the TAPA runtime library.

  The TAPA runtime library provides core functionality for TAPA tasks,
  streams, and memory maps. It implements platform-specific features
  (e.g., software simulation queues, hardware FIFOs).

- `tapa-system-include/`: Creates a custom system include directory for TAPA.

  This Bazel build target collects system include files for the `tapacc`
  compiler. It includes standard C++ headers, TAPA dependencies, and
  TAPA-specific headers for the compiler to run on every OS.

- `tapa-core/`: Contains the supported Rust TAPA compiler implementation.

  The TAPA compiler serves as the entry point for the TAPA framework. It
  invokes the `tapacc` compiler, synthesizes tasks into RTL
  using HLS tools, and generates system interconnect and XO object file for
  FPGA. For the `xilinx-hls` target, a `.zip` RTL archive is generated
  instead. The Bazel `//tapa-core:tapa` target provides the command wrapper
  and `tapa-core/tapa-codegen/assets` contains shared Verilog templates.

- `tapacc/`: Implements the TAPA C++ compiler to translate TAPA tasks to JSON.

  The TAPA C++ compiler is a Clang-based compiler for TAPA tasks. It parses
  every translation unit in one process, merges the task graph, writes the
  rewritten source tree that synthesis compiles, and emits the JSON
  representation of tasks and dataflow.

- `tests/`: Includes test cases for the TAPA compiler and runtime library.

  The tree includes small applications under `apps/`, focused end-to-end
  feature tests under `functional/`, and larger real designs under
  `regression/`.

## Test Coverage

Line coverage of the Rust workspaces is measured with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov):

```bash
cd tapa-core && cargo llvm-cov --locked --workspace
cd fpga-runtime && cargo llvm-cov --locked --workspace -- --skip open_cosim_verilator
```

The `open_cosim_verilator` end-to-end tests are skipped because they need
the `libfrt_dpi_verilator` shared library that only the Bazel runfiles
tree stages; run them through `bazel test //fpga-runtime:cargo_test`.

Coverage is informational, not a project target or CI gate. Remote SSH and
XRT paths may require a remote host or board, so use the Bazel integration
tests for those paths when the required resources are available.

## Update Dependencies

TAPA depends on several external libraries and tools. This section explains
how to update these dependencies.

### General Version Bump Process

When bumping versions, update the dependency declarations, regenerate only
the affected lock data, review the resulting changes, and run the required
tests. Do not clear every lock file by default.

### Bazel Dependencies

For Bazel dependencies:

1. Update the version numbers in `MODULE.bazel`.

2. Check the [Bazel Central Registry](https://registry.bazel.build/)
   for latest versions, and update the `bazel_dep` entries in
   `MODULE.bazel` accordingly.

3. Run the build so Bazel refreshes `MODULE.bazel.lock` as needed. Remove
   the lock file only when a clean regeneration is specifically required.

### LLVM Version Updates

TAPA has two separate LLVM configurations:

- The `tapacc` source dependency uses `LLVM_VERSION_MAJOR`,
  `LLVM_VERSION_MINOR`, `LLVM_VERSION_PATCH`, and `LLVM_SHA256` in
  `MODULE.bazel` (currently 23.1.2). Update these together, review the local
  patches under `bazel/patches/tapa-llvm-project/`, and verify the new source
  archive hash.
- The Bazel C/C++ action toolchain uses the archives and `toolchains_llvm`
  settings near the top of `MODULE.bazel` (currently 23.1.2). Follow
  `tools/llvm-toolchain/README.md` when changing that toolchain; its version
  need not be identical to the `tapacc` source dependency.

### Docker Images

For the Docker testing and building environments:

1. Update the base image versions in `.github/docker/*`.

2. Update the system dependencies trigger date to the current date, so that
   the Docker image is rebuilt with the latest system dependencies:

   ```dockerfile
   RUN apt-get update && \
       # Update the following line to retrigger the Docker build
       echo "Installing system dependencies as of <current-date>" && \
       apt-get upgrade -y
   ```

### Pre-commit Hooks

Update pre-commit hooks to the latest versions:

```bash
pre-commit autoupdate
```

### Verifying Updates

After updating dependencies:

1. Review the dependency and lock-file changes.

2. Run `bazel test //...`.

3. Run `pre-commit run --all-files`.

4. Commit with a Conventional subject and the required trailer, for example:

   ```bash
   git commit -a -m "build(deps): bump versions" \
     -m "Co-Authored-By: Kimi K3.1 Pro <noreply@moonshot.ai>"
   ```

```admonish note
This section provides guidance on updating all types of dependencies in the
TAPA project, including where to find the latest versions and how to verify
that the updates work correctly.
```
