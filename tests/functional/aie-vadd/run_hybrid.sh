#!/bin/bash
# Hybrid cosimulation: the PL half as RTL, the AI Engine task in software,
# driven by the unchanged host binary.
#
# usage: run_hybrid.sh TAPA HOST KERNEL PLATFORM [TOP]
set -euo pipefail

tapa="$1"
host="$2"
kernel="$3"
platform="$4"
top="${5:-VecAdd}"

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-hybrid.XXXXXX")"
"${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t "${top}" \
  --platform "${platform}" --skip-aie -o "${work}/aie.xo"
"${host}" --bitstream="${work}/aie.xo" 64
