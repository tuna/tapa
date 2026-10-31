#!/bin/bash
# VCK5000 hardware: build the graph, PL and PL controller, then run twice.
#
# usage: run_vck5000.sh TAPA HOST KERNEL PLATFORM [BDF [TOP]]
set -euo pipefail

tapa="$1"
host="$2"
kernel="$3"
platform="$4"
bdf="${5:-c1:00.1}"
top="${6:-VecAdd}"

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-vck5000.XXXXXX")"
"${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t "${top}" \
  --platform "${platform}" -o "${work}/aie.xo"
"${tapa}" --work-dir "${work}/work" link --package "${work}/aie.xo" \
  -o "${work}/aie.xclbin"
"${host}" --bitstream="${work}/aie.xclbin" --xocl_bdf="${bdf}" 64
