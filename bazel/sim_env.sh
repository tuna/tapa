#!/bin/bash
# Simple passthrough for running TAPA simulation tests without Xilinx tools.

if [[ -n "${VERILATOR_BIN:-}" && -z "${VERILATOR_ROOT:-}" ]]; then
  bin_dir="$(dirname "$VERILATOR_BIN")"
  if [[ "$bin_dir" = /* ]]; then
    bazel_root="$bin_dir"
  else
    bazel_root="$(cd "$PWD/$bin_dir" && pwd)"
  fi

  # Prefer the Bazel-managed verilator root if it is complete.
  if [[ -f "$bazel_root/include/verilated.mk" && -f "$bazel_root/bin/verilator_includer" ]]; then
    export VERILATOR_ROOT="$bazel_root"
  else
    # Otherwise fall back to a system installation.
    for sys_root in /opt/homebrew/share/verilator /usr/local/share/verilator /usr/share/verilator; do
      if [[ -f "$sys_root/include/verilated.mk" && -f "$sys_root/bin/verilator_includer" ]]; then
        export VERILATOR_ROOT="$sys_root"
        break
      fi
    done
  fi
fi

exec "$@"
