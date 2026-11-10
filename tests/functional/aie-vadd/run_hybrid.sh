#!/bin/bash
# Hybrid cosimulation: the PL half as RTL, the AI Engine task in software,
# driven by the unchanged host binary.
#
# usage: run_hybrid.sh TAPA HOST KERNEL PLATFORM [TOP]
# HOST runs with HYBRID_HOST_ARG as its argument (default 64).
set -euo pipefail

tapa="$1"
host="$2"
kernel="$3"
platform="$4"
top="${5:-VecAdd}"
shift $(($# < 5 ? $# : 5))  # the rest are extra `tapa compile` flags

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-hybrid.XXXXXX")"
"${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t "${top}" \
  --platform "${platform}" --skip-aie -o "${work}/aie.xo" "$@"
"${host}" --bitstream="${work}/aie.xo" "${HYBRID_HOST_ARG:-64}"
