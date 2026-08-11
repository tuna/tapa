# Installation

**Purpose:** Install TAPA on your development machine.

**When to use this:** Setting up TAPA for the first time.

## What you need

| Dependency | Version | Notes |
|------------|---------|-------|
| GNU C++ Compiler (`g++`) | 7.5.0 or newer | Required for software simulation and deployment |
| Xilinx Vitis | 2022.1 or newer | **Not needed for software simulation** — only required for RTL synthesis and deployment |

TAPA has been tested on the following operating systems:

| OS | Minimum version | Notes |
|----|-----------------|-------|
| Ubuntu | 18.04 | |
| Debian | 10 | |
| Red Hat Enterprise Linux | 9 | Derivatives (AlmaLinux 9+, Rocky Linux 9+) also supported |
| Amazon Linux | 2023 | |
| Fedora | 34 | Fedora 39+ may have minor issues due to C library changes and Vitis HLS incompatibility |

## Install from release

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
```

This installs the latest release. With root privileges, TAPA installs to
`/opt/tapa` with symlinks in `/usr/local/bin`. Otherwise it installs to
`~/.tapa` and adds itself to your `PATH` via your shell profile. To update to
a newer release, run the script again — it overwrites the existing
installation.

To install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh \
  | TAPA_VERSION=0.1.20260319 sh -s -- -q
```

Releases are available at [github.com/tuna/tapa/releases](https://github.com/tuna/tapa/releases).

## Install g++

Install `g++` using the package manager for your OS.

### Ubuntu / Debian

For Ubuntu 18.04 and newer, or Debian 10 and newer:

```bash
sudo apt-get install g++
```

### RHEL / Amazon Linux

For Red Hat Enterprise Linux 9 and newer, derivatives like AlmaLinux 9 and newer
and Rocky Linux 9 and newer, or Amazon Linux 2023:

```bash
sudo yum install gcc-c++ libxcrypt-compat
```

### Fedora

For Fedora 34 and newer. Fedora 39 and newer may have minor issues due to system
C library changes and Vitis HLS tool incompatibility.

```bash
sudo yum install gcc-c++ libxcrypt-compat
```

## Install CBC

Floorplanning (`tapa floorplan` and synthesis flows that floorplan) solves its
ILPs with the external [CBC](https://github.com/coin-or/Cbc) solver binary,
which must be on `PATH`. Software simulation does not need it.

### Ubuntu / Debian

```bash
sudo apt-get install coinor-cbc
```

### RHEL / Fedora

```bash
sudo yum install coin-or-Cbc
```

## Running in a container

A minimal base image (for example `ubuntu:24.04`) lacks two things the Xilinx
tools need. Neither is a TAPA dependency — software simulation works without
them — but `tapa synth` and cosimulation shell out to Vitis HLS and Vivado,
and both fail with obscure errors when they are missing.

**A UTF-8 locale.** Vitis HLS aborts with
`locale::facet::_S_create_c_locale name not valid`:

```bash
apt-get install -y locales && locale-gen en_US.UTF-8
export LANG=en_US.UTF-8
```

**Vivado's runtime libraries.** Vivado fails to start with
`couldn't load file "libxv_tcltasks.so": libpixman-1.so.0: cannot open shared
object file`:

```bash
apt-get install -y libpixman-1-0 libtinfo6 libncurses6 libx11-6 libxext6 \
  libxrender1 libfontconfig1 libfreetype6
```

**Versal platforms additionally need `libyaml-0-2`.** Linking against a
Versal platform rebuilds the PLM firmware, and Vivado's `dtc` fails with
`libyaml-0.so.2: cannot open shared object file` when the library is
missing:

```bash
apt-get install -y libyaml-0-2
```

Mount the Xilinx installation and the platform repository into the container
and source the tool settings as usual:

```bash
docker run -it \
  -v /opt/Xilinx:/opt/Xilinx:ro \
  -v /opt/xilinx/platforms:/opt/xilinx/platforms:ro \
  ubuntu:24.04
source /opt/Xilinx/Vitis/<version>/settings64.sh
```

Running a design on hardware or in hardware emulation additionally needs XRT
installed inside the container; software simulation and fast cosimulation do
not. XRT's GitHub releases do not publish installable packages — download
the XRT `.deb`/`.rpm` for your distribution from the
[Xilinx download center](https://www.xilinx.com/support/download.html)
(bundled with the Vitis/XRT unified installer downloads) and install it with
your package manager, for example `apt-get install ./xrt_*-xrt.deb`.

## Verify installation

```bash
tapa --version
```

## Building from source

For source builds (full toolchain requirements and build commands), see [Building from Source](../developer/build.md).

```admonish warning
If installation fails, see [Common Errors](../troubleshoot/common-errors.md) for known issues.
```

**Next step:** [Your First Run](first-run.md)
