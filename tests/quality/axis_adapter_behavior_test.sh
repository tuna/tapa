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

# Verilator's --timing output needs C++20 coroutines, but the generated
# verilated.mk hard-codes `CXX = c++`, which may be too old (the CI image
# is bionic with gcc-7; g++-11 is installed there for this). Probe for a
# capable compiler and force it via MAKEFLAGS — make command-line
# variables beat makefile assignments. The <coroutine> include matters:
# gcc-10 accepts -std=gnu++20 but still gates coroutines behind a flag.
CXX_FOR_VERILATOR=""
for candidate in "${CXX:-}" c++ g++ g++-12 g++-11; do
  [[ -n "${candidate}" ]] || continue
  if printf '#include <coroutine>\nint main() {}\n' |
    "${candidate}" -x c++ -std=gnu++20 - -o /dev/null 2>/dev/null; then
    CXX_FOR_VERILATOR="${candidate}"
    break
  fi
done
if [[ -z "${CXX_FOR_VERILATOR}" ]]; then
  echo "FAIL: no C++20-coroutine-capable compiler for verilator --timing" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

"${VERILATOR}" \
  --binary \
  --assert \
  --sv \
  --timing \
  -Wno-fatal \
  -CFLAGS -std=gnu++20 \
  -MAKEFLAGS "CXX=${CXX_FOR_VERILATOR}" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/axis_adapter.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_fwd.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_srl.v)" \
  "$(resolve_runfile tapa-core/tapa-codegen/assets/verilog/fifo_bram.v)" \
  "$(resolve_runfile tests/quality/axis_adapter_smoke_tb.sv)" \
  --Mdir "${tmp}/obj_dir" \
  --top-module tb

"${tmp}/obj_dir/Vtb"
