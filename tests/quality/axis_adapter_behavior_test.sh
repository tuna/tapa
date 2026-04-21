#!/usr/bin/env bash
set -euo pipefail

if ! command -v verilator >/dev/null 2>&1; then
  echo "SKIP: verilator not available" >&2
  exit 0
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

verilator \
  --binary \
  --assert \
  --sv \
  --timing \
  -Wno-fatal \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/axis_adapter.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_fwd.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_srl.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_bram.v)" \
  "$(resolve_runfile tests/quality/axis_adapter_smoke_tb.sv)" \
  --Mdir "${tmp}/obj_dir" \
  --top-module tb

"${tmp}/obj_dir/Vtb"
