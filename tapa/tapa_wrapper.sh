#!/usr/bin/env bash
# Wrapper that makes `bazel run //tapa:tapa` expose the sibling tools
# (tapacc, tapa-cpp, tapa-system-include) that the native `tapa-cli`
# walks up to find via `find_resource`. Without this, the Rust binary
# would start in its own runfiles subtree and fail to discover the
# Clang-based front-ends.
set -euo pipefail

# --- begin runfiles.bash initialization v3 ---
# Canonical runfiles bootstrap; see
# https://github.com/bazelbuild/bazel/blob/master/tools/bash/runfiles/runfiles.bash
f=bazel_tools/tools/bash/runfiles/runfiles.bash
source "${RUNFILES_DIR:-/dev/null}/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "${RUNFILES_MANIFEST_FILE:-/dev/null}" | cut -f2- -d' ')" 2>/dev/null || \
  source "$0.runfiles/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  { echo >&2 "tapa wrapper: runfiles.bash not found"; exit 1; }
# --- end runfiles.bash initialization v3 ---

tapa_bin="$(rlocation _main/tapa-core/cargo/bin/tapa)"
if [[ -z "${tapa_bin}" || ! -x "${tapa_bin}" ]]; then
  # Repo-name variant used when the main repo is not `_main`.
  tapa_bin="$(rlocation tapa/tapa-core/cargo/bin/tapa)"
fi
if [[ -z "${tapa_bin}" || ! -x "${tapa_bin}" ]]; then
  echo >&2 "tapa wrapper: cannot locate tapa-cli binary in runfiles"
  exit 1
fi

# Anchor `find_resource` at the runfiles copy of the tapa binary so its
# parent walk reaches `<runfiles>/<workspace>/` and resolves siblings
# `tapacc/tapacc`, `tapa-cpp/tapa-cpp`, and
# `tapa-system-include/tapa-system-include`.
export TAPA_CLI_SEARCH_ANCHOR="${tapa_bin}"

exec "${tapa_bin}" "$@"
