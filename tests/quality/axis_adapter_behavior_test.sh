#!/usr/bin/env bash
set -euo pipefail

# Under Bazel, VERILATOR_BIN points at the hermetic @verilator binary
# (staged in runfiles); outside Bazel, fall back to PATH. Unset the
# variable after capturing it: Verilator's own Perl frontend consumes
# VERILATOR_BIN to pick its backend, so leaving it pointed at a wrapper
# would make the wrapper re-exec itself forever.
VERILATOR="${VERILATOR_BIN:-}"
unset VERILATOR_BIN
if [[ -z "${VERILATOR}" ]]; then
  if ! command -v verilator >/dev/null 2>&1; then
    echo "SKIP: verilator not available" >&2
    exit 0
  fi
  VERILATOR=verilator
fi

resolve_runfile() {
  local path="$1"
  local workspace="${TEST_WORKSPACE:-_main}"
  local candidate
  for candidate in \
    "${RUNFILES_DIR:-}/${workspace}/${path}" \
    "${RUNFILES_DIR:-}/_main/${path}" \
    "${RUNFILES_DIR:-}/${path}" \
    "$0.runfiles/${workspace}/${path}" \
    "$0.runfiles/_main/${path}" \
    "$0.runfiles/${path}"; do
    if [[ -f "${candidate}" ]]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done
  if [[ -n "${RUNFILES_MANIFEST_FILE:-}" ]]; then
    for candidate in "${workspace}/${path}" "_main/${path}" "${path}"; do
      local resolved
      resolved="$(grep -m1 "^${candidate} " "${RUNFILES_MANIFEST_FILE}" | cut -d' ' -f2- || true)"
      if [[ -n "${resolved}" && -f "${resolved}" ]]; then
        printf '%s\n' "${resolved}"
        return 0
      fi
    done
  fi
  echo "missing runfile: ${path}" >&2
  return 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

# --no-timing: the stimulus lives in the C++ harness, so the model has no
# delay controls to schedule. That keeps the generated code buildable by the
# CI image's gcc-7 (Verilator's --timing output needs C++20 coroutines, and
# the generated verilated.mk hard-codes `CXX = c++`).
"${VERILATOR}" \
  --cc \
  --exe \
  --build \
  --assert \
  --sv \
  --no-timing \
  -Wno-fatal \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/axis_adapter.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_fwd.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_srl.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_bram.v)" \
  "$(resolve_runfile tests/quality/axis_adapter_smoke_tb.sv)" \
  "$(resolve_runfile tests/quality/axis_adapter_smoke_main.cpp)" \
  --Mdir "${tmp}/obj_dir" \
  --top-module tb

"${tmp}/obj_dir/Vtb"
