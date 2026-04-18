#!/bin/bash
# End-to-end flag regression: run `tapa analyze synth pack` on
# `tests/apps/vadd` twice — once with `TAPA_USE_RUST_XILINX=1`, once
# without — and assert the produced `.xo` archives are semantically
# equal under the same redaction pass we apply to reproducible builds.
#
# Skips cleanly (exit 0) when neither a local Xilinx install nor a
# configured remote host is available; this script is intended to run
# from the gated `tapa_xilinx_integration_test` Bazel target, which
# already loads `VARS.local.bzl` and the local `XILINX_HLS`.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
vadd_dir="$repo_root/tests/apps/vadd"

if [[ ! -d "$vadd_dir" ]]; then
  echo "vadd fixture missing at $vadd_dir" >&2
  exit 2
fi

if [[ -z "${XILINX_HLS:-}" ]] && [[ -z "${REMOTE_HOST:-}" ]]; then
  echo "vadd flag-parity: neither XILINX_HLS nor REMOTE_HOST is set; skipping" >&2
  exit 0
fi

# REMOTE-only setups must also supply the auth key + xilinx settings
# path — the tapa CLI needs these as explicit `--remote-*` args, since
# Python's `load_remote_config(None)` does not read REMOTE_* env vars.
# Without these, the script would fall through to the local path and
# silently skip the flagged remote flow.
#
# `REMOTE_XILINX_SETTINGS` (absolute settings script path) wins over
# `REMOTE_XILINX_TOOL_PATH` (tool-root dir). If only the tool-root is
# set, we normalize it to `<root>/settings64.sh` — the Rust
# `resolve_xilinx_settings` helper uses the same rule. Handing the
# remote runner a bare directory makes `source <dir>` fail silently
# at bash-time, so we refuse to run in that state.
REMOTE_CLI_ARGS=()
if [[ -z "${XILINX_HLS:-}" && -n "${REMOTE_HOST:-}" ]]; then
  settings=""
  if [[ -n "${REMOTE_XILINX_SETTINGS:-}" ]]; then
    settings="$REMOTE_XILINX_SETTINGS"
  elif [[ -n "${REMOTE_XILINX_TOOL_PATH:-}" ]]; then
    case "$REMOTE_XILINX_TOOL_PATH" in
      *.sh) settings="$REMOTE_XILINX_TOOL_PATH" ;;
      *) settings="${REMOTE_XILINX_TOOL_PATH%/}/settings64.sh" ;;
    esac
  fi
  if [[ -z "${REMOTE_KEY_FILE:-}" || -z "$settings" ]]; then
    echo "vadd flag-parity: REMOTE_HOST set but REMOTE_KEY_FILE / REMOTE_XILINX_SETTINGS (or _TOOL_PATH) missing; skipping" >&2
    exit 0
  fi
  REMOTE_CLI_ARGS+=(--remote-host "$REMOTE_HOST")
  REMOTE_CLI_ARGS+=(--remote-key-file "$REMOTE_KEY_FILE")
  REMOTE_CLI_ARGS+=(--remote-xilinx-settings "$settings")
  if [[ -n "${REMOTE_SSH_CONTROL_DIR:-}" ]]; then
    REMOTE_CLI_ARGS+=(--remote-ssh-control-dir "$REMOTE_SSH_CONTROL_DIR")
  fi
  if [[ -n "${REMOTE_SSH_CONTROL_PERSIST:-}" ]]; then
    REMOTE_CLI_ARGS+=(
      --remote-ssh-control-persist "$REMOTE_SSH_CONTROL_PERSIST"
    )
  fi
fi

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

PLATFORM="${VADD_PARITY_PLATFORM:-xilinx_u250_gen3x16_xdma_4_1_202210_1}"
TOP="${VADD_PARITY_TOP:-VecAdd}"

# Resolve the `tapa` binary preferring (1) an explicit `$TAPA_BIN`,
# (2) a Bazel runfiles-local copy (matches `tests/apps/analyze_test.py::_find_tapa`),
# (3) the system `tapa` on PATH. Failing those, skip cleanly so the
# harness never accidentally runs against a broken PATH install.
TAPA_BIN_RESOLVED=""
if [[ -n "${TAPA_BIN:-}" && -x "$TAPA_BIN" ]]; then
  TAPA_BIN_RESOLVED="$TAPA_BIN"
else
  for base in "${RUNFILES_DIR:-}" "${TEST_SRCDIR:-}"; do
    if [[ -n "$base" && -x "$base/_main/tapa/tapa" ]]; then
      TAPA_BIN_RESOLVED="$base/_main/tapa/tapa"
      break
    fi
  done
fi
if [[ -z "$TAPA_BIN_RESOLVED" ]]; then
  if command -v tapa >/dev/null 2>&1; then
    TAPA_BIN_RESOLVED="$(command -v tapa)"
  fi
fi
if [[ -z "$TAPA_BIN_RESOLVED" ]]; then
  echo "vadd flag-parity: no tapa binary found (TAPA_BIN / runfiles / PATH); skipping" >&2
  exit 0
fi
# Sanity check: the resolved binary must actually work. A broken
# system install (`/opt/homebrew/bin/tapa` with a missing
# pkg_resources, for example) must trigger a clean skip rather than
# a false pass.
if ! "$TAPA_BIN_RESOLVED" --help >/dev/null 2>&1; then
  echo "vadd flag-parity: resolved tapa binary ($TAPA_BIN_RESOLVED) does not run cleanly; skipping" >&2
  exit 0
fi

# `tapacc-binary` preflight: delegate to a hidden `find-clang-binary`
# subcommand inside `$TAPA_BIN_RESOLVED` itself. That guarantees the
# preflight and the real `tapa analyze` run share the same launcher
# runtime (interpreter, `sys.path`, runfiles). Previous shebang-
# parsing approaches fell back to ambient `python3` for shell/Bazel
# wrappers, which could import a different `tapa` package than the
# launcher will at analyze-time. Running the resolver through the
# launcher closes that gap.
tapacc_probe_rc=0
tapacc_probe_out="$("$TAPA_BIN_RESOLVED" find-clang-binary tapacc-binary)" \
  || tapacc_probe_rc=$?
if [[ $tapacc_probe_rc -ne 0 || -z "$tapacc_probe_out" ]]; then
  echo "vadd flag-parity: 'tapa find-clang-binary tapacc-binary' exited $tapacc_probe_rc; skipping" >&2
  exit 0
fi
TAPACC_BIN_RESOLVED="$tapacc_probe_out"

run_one() {
  local label="$1"
  local flag_value="$2"
  local outdir="$workdir/$label"
  mkdir -p "$outdir"
  (
    cd "$vadd_dir"
    env TAPA_USE_RUST_XILINX="$flag_value" \
      "$TAPA_BIN_RESOLVED" \
        --work-dir "$outdir/work" \
        "${REMOTE_CLI_ARGS[@]}" \
        analyze \
          --input vadd.cpp \
          --top "$TOP" \
        synth \
          --platform "$PLATFORM" \
        pack \
          --output "$outdir/vadd.xo"
  )
  if [[ ! -s "$outdir/vadd.xo" ]]; then
    echo "vadd flag-parity: $label run produced no .xo at $outdir/vadd.xo" >&2
    return 1
  fi
}

echo "running vadd unflagged (Python path) ..." >&2
run_one unflagged "0"

echo "running vadd flagged (Rust path) ..." >&2
run_one flagged "1"

# Compare the two archives' listings + per-file content hashes. The
# `.xo` redaction already normalizes timestamps and source paths, so
# any residual difference is a real regression.
list_and_hash() {
  local xo="$1"
  python3 - <<PY "$xo"
import hashlib, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    for info in sorted(z.infolist(), key=lambda i: i.filename):
        h = hashlib.sha256(z.read(info)).hexdigest()
        print(f"{info.filename} {info.file_size} {h}")
PY
}

diff \
  <(list_and_hash "$workdir/unflagged/vadd.xo") \
  <(list_and_hash "$workdir/flagged/vadd.xo")
echo "vadd flag-parity: .xo archives are semantically equal" >&2
