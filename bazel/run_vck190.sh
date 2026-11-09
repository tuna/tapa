#!/bin/bash
# VCK190 system emulation of a design whose top runs on the AI Engine:
# `tapa compile` packs it as a `.zip`, `tapa link --sim` links it for
# hardware emulation, `v++ --package` makes the SD image with the aarch64
# host program, and QEMU boots it and runs the host twice, so the second
# run reuses the transfers the first left posted.
#
# usage: run_vck190.sh TAPA HOST KERNEL TOP PLATFORM
# VERSAL_COMMON must name the extracted xilinx-versal-common directory.
set -euo pipefail
export PATH="${PATH}:/usr/sbin:/sbin"
# `//bazel:v++_env.sh` puts the host's glibc headers and libraries first for
# xsim; the AI Engine compiler's bundled gcc and the MicroBlaze toolchain
# that builds the Versal boot firmware cannot build against them.
unset CPLUS_INCLUDE_PATH C_INCLUDE_PATH LIBRARY_PATH

tapa="$1"
host="$2"
kernel="$3"
top="$4"
platform="$5"
common="${VERSAL_COMMON:?set VERSAL_COMMON to the extracted Versal common-image directory}"

for input in "${common}/Image" "${common}/rootfs.ext4"; do
  if [[ ! -f "${input}" ]]; then
    echo "missing Versal common-image input: ${input}" >&2
    exit 1
  fi
done

work="$(mktemp -d "${TEST_TMPDIR:-/tmp}/aie-vck190.XXXXXX")"
host_name="$(basename "${host}")"
xclbin="${top}.hw_emu.xclbin"
"${tapa}" --work-dir "${work}/work" compile -f "${kernel}" -t "${top}" \
  --platform "${platform}" -o "${work}/${top}.zip"
"${tapa}" --work-dir "${work}/work" link --package "${work}/${top}.zip" --sim \
  -o "${work}/${xclbin}"

cd "${work}"
unzip -o -q "${top}.zip" libadf.a
v++ --package --target hw_emu --platform "${platform}" \
  --package.boot_mode sd --package.defer_aie_run \
  --package.kernel_image "${common}/Image" \
  --package.rootfs "${common}/rootfs.ext4" \
  --package.sd_file "${host}" --package.sd_file "${xclbin}" \
  --package.out_dir package --temp_dir package.tmp \
  --output package/sd.xclbin "${top}.hw_emu.xsa" libadf.a
cd package
run="./${host_name} --bitstream=${xclbin}"
# The emulator stops at the first line holding the result string, so it is
# printed once both runs pass, spelled so the echoed command lacks it.
timeout --kill-after=30 "${AIE_HW_EMU_TIMEOUT:-7200}" ./launch_hw_emu.sh \
  -run-app "${run} && ${run} && echo TAPA_RUNS_\$((1 + 1))_PASSED; poweroff -f" \
  -result-string "TAPA_RUNS_2_PASSED"
