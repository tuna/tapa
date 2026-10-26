#!/bin/bash
# VCK190 system emulation: PL RTL, AI Engine, and the aarch64 XRT host.
#
# usage: run_vck190.sh TAPA HOST KERNEL PLATFORM
# VERSAL_COMMON must name the extracted xilinx-versal-common directory.
set -euo pipefail
export PATH="${PATH}:/usr/sbin:/sbin"

tapa="$1"
host="$2"
kernel="$3"
platform="$4"
common="${VERSAL_COMMON:?set VERSAL_COMMON to the extracted Versal common-image directory}"

for input in "${common}/Image" "${common}/rootfs.ext4"; do
  if [[ ! -f "${input}" ]]; then
    echo "missing Versal common-image input: ${input}" >&2
    exit 1
  fi
done

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-vadd-vck190.XXXXXX")"
host_name="$(basename "${host}")"
env -u CPLUS_INCLUDE_PATH -u C_INCLUDE_PATH \
  "${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t VecAdd \
  --platform "${platform}" -o "${work}/vadd.xo" -s "${work}/link.sh" \
  --package-kernel-image "${common}/Image" \
  --package-rootfs "${common}/rootfs.ext4" \
  --package-sd-file "${host}"

cd "${work}"
env -u CPLUS_INCLUDE_PATH -u C_INCLUDE_PATH TARGET=hw_emu ./link.sh
xclbin="VecAdd_${platform}.xclbin"
timeout --kill-after=30 "${AIE_HW_EMU_TIMEOUT:-7200}" ./launch_hw_emu.sh \
  -run-app "./${host_name} --bitstream=${xclbin} 64 && ./${host_name} --bitstream=${xclbin} 64; poweroff -f" \
  -result-string "PASS!"
