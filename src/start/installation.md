# Installation

**Purpose:** Install TAPA on your development machine.

**When to use this:** Setting up TAPA for the first time.

## What you need

| Dependency | Version | Notes |
|------------|---------|-------|
| GNU C++ Compiler (`g++`) | 7.5.0 or newer | Required for software simulation |
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

This downloads and installs the latest release. With root privileges, TAPA
installs to `/opt/tapa` with symlinks in `/usr/local/bin`. Otherwise it installs
to `~/.tapa` and adds itself to your `PATH` via your shell profile.

To install a specific version:

```bash
TAPA_VERSION=0.1.20260319 \
  curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
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

## Verify installation

```bash
tapa --version
```

## Building from source

For source builds, see [Building from Source](../developer/build.md).

```admonish warning
If installation fails, see [Common Errors](../troubleshoot/common-errors.md) for known issues.
```

**Next step:** [Your First Run](first-run.md)
