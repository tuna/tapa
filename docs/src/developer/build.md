# Building from Source

```admonish note
This guide is for **developers** contributing to or extending TAPA,
or **advanced users** building TAPA from source for custom OS support.
For FPGA accelerator development with TAPA, use the release installer in
[Installation](../start/installation.md) unless you specifically need a
source build.
```

```admonish tip
If your OS isn't officially supported, consider using a virtual machine
or file a [feature request on GitHub](https://github.com/tuna/tapa/issues).
```

## System Prerequisites

To build TAPA from source, you need:

- [Bazel](https://bazel.build) 7.3.2 or later
- [Binutils](https://www.gnu.org/software/binutils/) 2.30 or later
- [Git](https://git-scm.com)
- C library headers, startup files, and libraries for your system
- [Libstdc++](https://gcc.gnu.org/libstdc++/) headers and libraries matching
  the most recent GCC version installed on your system
- [Python](https://www.python.org) 3 or later for Bazel
- [Rust](https://www.rust-lang.org) 1.99.1 (via [rustup](https://rustup.rs)) for
  building via `cargo` directly — pre-commit's fmt/clippy hooks use it. The
  version is pinned in `rust-toolchain.toml` and must match the Bazel toolchain
  pinned in `MODULE.bazel`; bump both together.
- [Other TAPA dependencies](../start/installation.md)

Install these tools using your OS package manager. For Ubuntu:

```bash
# Install bazel
sudo apt-get install apt-transport-https ca-certificates gnupg
curl -fsSL https://releases.bazel.build/bazel-release.pub.gpg \
  | gpg --dearmor | sudo tee /usr/share/keyrings/bazel-archive-keyring.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/bazel-archive-keyring.gpg] \
    https://storage.googleapis.com/bazel-apt stable jdk1.8" \
  | sudo tee /etc/apt/sources.list.d/bazel.list
sudo apt-get update
sudo apt-get install bazel

# Install Rust (rustup honors the version pinned in rust-toolchain.toml)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install other tools
sudo apt-get install g++ binutils git python3 coinor-cbc
```

```admonish tip
For Bazel installation on other OS, see the
[Bazel documentation](https://docs.bazel.build/versions/main/install.html).
```

```admonish note
The [Dockerfile in the TAPA repository](https://github.com/tuna/tapa/blob/main/.github/docker/build-env/Dockerfile.binary-dependencies)
provides a complete build environment. Use it for containerized builds or
run the Ubuntu commands to install required tools.
```

## Clone the Repository

To get started with building TAPA from source, you'll need to clone the repository from GitHub:

```bash
git clone https://github.com/tuna/tapa.git
cd tapa
```

If you are contributing to TAPA, fork the repository and clone your fork
instead. When you're ready to contribute, create a new branch for your
changes, commit your work, and open a pull request to contribute your
changes back to the main repository.

## Modify the Build Configuration

Create a git-ignored `VARS.local.bzl` in the repository root for local
overrides; do not edit `VARS.bzl`, which holds the shared defaults. The
default tool installation is `/opt/tools/xilinx` with version `2024.2`, so
the expected paths are `/opt/tools/xilinx/Vivado/2024.2` and
`/opt/tools/xilinx/Vitis/2024.2`.

If your tools are elsewhere, set `XILINX_TOOL_PATH` and
`XILINX_TOOL_VERSION` in `VARS.local.bzl`. Both classic
`<root>/<Tool>/<version>` and AMD unified `<root>/<version>/<Tool>` layouts
are probed. If XRT is unavailable, set `HAS_XRT = False` there to mark
XRT-dependent test targets incompatible instead of failing them. Bazel also
probes the HLS headers and runner and the Vivado executable; targets that
require unavailable tools are marked incompatible and skipped, while host,
unit, and tapacc analysis tests remain available. Remote configurations stay
enabled. See `bazel/vars.bzl` for the override mechanism and `VARS.bzl` for
all available settings.

The full analyze -> synth -> pack -> cosim chain also has a vendor-free
tier: `//tests/apps/vadd:vadd-fixture-zipsim` synthesizes against
checked-in HLS-output fixtures (`tapa_xo` with `hls_fixtures`, which runs
`tapa synth --skip-hls-based-on-mtime` over the staged fixtures) and
simulates the resulting zip with frt-cosim's Verilator backend, so TAPA's
own analyze, codegen, pack, and cosim are exercised end-to-end on hosts
with no Xilinx tools at all. The fixtures and their interface contract are
documented in `tests/apps/vadd/hls-fixtures/PROVENANCE.md`.

## Build TAPA from Source

To build TAPA, navigate to the root directory of the cloned repository and execute the following command:

```bash
bazel build //...
```

This command compiles all TAPA targets, including the compiler, runtime
library, and tests.

On Linux x86_64, the compiler, linker, and binary utilities used by Bazel C/C++
actions are static LLVM 23.1.2 binaries from the published hermetic-llvm
archive. Headers and compiler-rt libraries come from the same version's
official archive. These action tools run without host libxml2, zlib, or
libstdc++ shared libraries. Retained auxiliary LLVM programs keep their
upstream runtime dependencies.
Generated programs still use the system's C and C++ headers and libraries;
this is not a bundled sysroot. Other execution platforms use the distributions
selected by `toolchains_llvm`. The LLVM source dependency used to build
`tapacc` is pinned separately in `MODULE.bazel` (currently 23.1.2). See the
[toolchain details](https://github.com/tuna/tapa/tree/main/tools/llvm-toolchain)
for archive provenance and the optional source build.

Hardware-emulation builds explicitly select XSIM's bundled Clang for generated
simulation code, rather than relying on the host GCC's default C standard.
SystemC compilation and linking use AMD's bundled GCC and libraries.

For building a specific target, replace `//...` with the desired target
name. For instance, to build only the TAPA compiler:

```bash
bazel build //tapa-core:tapa
```

```admonish note
To view all available targets, run `bazel query //...`.
```

To skip targets under `tests/`, use:

```bash
bazel build -- //... -//tests/...
```

After the build process completes, you can find the compiled binaries in the
`bazel-bin` directory. For example, the TAPA compiler binary is located at
`bazel-bin/tapa-core/tapa`.

```admonish note
The build process duration may vary depending on your system's performance.
LLVM, a significant dependency used by TAPA for code generation, requires
considerable time to build. Bazel will cache it after the initial build.
```

## Use the Built TAPA

```admonish important
Source the Vitis/Vivado settings script before commands that invoke those
tools. It is not needed for `tapa analyze` or software simulation.
```

Once TAPA is built, you can use the compiled TAPA compiler to compile your
designs. For example:

```bash
bazel-bin/tapa-core/tapa compile \
 -f tests/apps/bandwidth/bandwidth.cpp \
 --cflags -Itests/apps/bandwidth/ \
 -t Bandwidth \
 --clock-period 3 \
 --part-num xcu250-figd2104-2L-e
```

Remember to rerun the `bazel build` command whenever you make changes to the
TAPA compiler or runtime library to ensure you're using the latest version.

## Build the Documentation

The documentation is written in Markdown and built with [mdBook](https://rust-lang.github.io/mdBook/). The Bazel build rules fetch the correct mdBook and mdbook-admonish binaries automatically — no separate install is needed.

**Build a static HTML site:**

```bash
bazel build //docs:build
```

The output is a tarball at `bazel-bin/docs/book.tar.gz`. Extract it to browse the HTML locally.

**Serve with live reload during editing:**

```bash
bazel run //docs:serve
```

This starts a local server (default: `http://localhost:3000`) that reloads automatically when source files change. Supported on Linux x86\_64, macOS x86\_64, and macOS arm64.

```admonish note
The documentation source lives under `docs/src/`. The Bazel targets handle mdbook-admonish preprocessing automatically; do not run `mdbook-admonish install` manually in the source tree.
```

## Run TAPA Tests

To run all TAPA tests, including unit tests and integration tests, use the
following command in the repository's root directory:

```bash
bazel test //...
```

For running a specific test, replace `//...` with the test name. For example,
to test only a specific app:

```bash
bazel test //tests/apps/vadd:vadd-xosim
```

## Build Binary Distribution

To create a binary distribution of TAPA, navigate to the root directory of the
cloned repository and execute the following command:

```bash
bazel build --config=release //:tapa-pkg-tar
```

Find the generated binary distribution in the `bazel-bin` directory,
as a tarball named `tapa-pkg-tar.tar`.

## Install the Binary Distribution

To install the binary distribution, extract the tarball to a directory of your
choice:

```bash
tar -xvf bazel-bin/tapa-pkg-tar.tar -C /path/to/install
```

Access the TAPA compiler binary at `/path/to/install/usr/bin/tapa`.

## Containerized Build (Advanced)

For those who prefer a containerized build environment, TAPA offers a GitHub
Actions workflow that can be run locally using `act`. This approach ensures
a consistent build environment across different systems.

### Prerequisites

1. Install `act` by following the instructions in the
   [act repository](https://nektosact.com).

2. Ensure Docker is installed on your system, as `act` requires it to run
   the workflow.

### Configuration

Create a git-ignored `.ci-secrets` file in the repository root (the filename
already referenced by `.actrc`):

```text
UBUNTU_PRO_TOKEN=[YOUR_UBUNTU_PRO_TOKEN]
MAC_ADDRESS=de:ed:be:ef:ca:fe
```

Replace the placeholders with your Ubuntu Pro token and the MAC address used
by your Vivado license. If your license and installation locations differ
from the defaults (`/share/software/licenses/xilinx-ci.lic` and
`/share/software/tools`), adjust them for your local run without committing
those local changes.

### Running Containerized Tests

To test TAPA in the containerized environment:

```bash
act -W .github/workflows/dev-branches.yml -j test
```

This method often provides more consistent results than local testing due to
the isolated environment. It also benefits from a shared Bazel cache between
runs, potentially speeding up the build process.

```admonish note
Build artifacts are not saved to the local `bazel-bin` directory in
containerized builds. For debugging, you may need to build TAPA in your
local environment. However, you can still add test cases and use `act`
for testing your changes.
```

### Creating a Binary Distribution

To create a binary distribution of TAPA:

```bash
act -W .github/workflows/staging-build.yml -j build
```

Act writes uploaded artifacts under `artifacts.out`; inspect its output for
the exact `tapa.tar.gz` path.

### Installing the Binary Distribution

To install the binary distribution:

1. Extract the tarball to your preferred directory, or
2. Use the provided `install.sh` script to install TAPA to the default
   location:

   ```bash
   TAPA_LOCAL_PACKAGE=/path/to/tapa.tar.gz ./install.sh
   ```
