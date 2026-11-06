#!/bin/bash
# Builds the AI Engine graph of a design as a simulator package and runs the
# host program against it: for x86sim, the PL tasks in software and every
# PL <-> AI Engine stream relayed live; for aiesim, the whole design in
# software, then the simulator replaying its streams.
#
# usage: run_aie_sim.sh TAPA HOST KERNEL TOP SIMULATOR [compile flags...]
set -euo pipefail

tapa="$1"
host="$2"
kernel="$3"
top="$4"
simulator="$5"
shift 5

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-sim.XXXXXX")"
# `//bazel:v++_env.sh` puts the host's glibc headers first for xsim; the AI
# Engine compiler's bundled gcc 8.3 cannot build x86sim against them.
env -u CPLUS_INCLUDE_PATH -u C_INCLUDE_PATH \
  "${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t "${top}" \
  --target xilinx-vitis --part-num xcvc1902-vsva2197-2MP-e-S \
  --aie-sim "${simulator}" -o "${work}/graph.${simulator}" "$@"
env -u CPLUS_INCLUDE_PATH -u C_INCLUDE_PATH \
  "${host}" --bitstream="${work}/graph.${simulator}"
