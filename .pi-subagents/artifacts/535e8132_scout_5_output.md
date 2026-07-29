# Code Context

## Files Retrieved
1. `VARS.bzl` (lines 1-19) — defines Xilinx tool paths/versions, including legacy defaults.
2. `MODULE.bazel` (lines 1-260) — Bazel module graph and external repo wiring.
3. `bazel/dependencies.bzl` (lines 1-340) — tool/version resolution for local Vitis/Vivado/XSIM repos.
4. `bazel/BUILD.bazel` (lines 1-60) — wrapper and XSIM wrapper targets.
5. `bazel/dpi_rules.bzl` (lines 1-170) — DPI rule implementations including legacy variant.
6. `.github/workflows/staging-build.yml` (lines 1-155) — CI matrix and staging job graph.
7. `.github/workflows/publish-release.yml` (lines 1-70), `.github/workflows/dev-branches.yml` (1-130), `.github/workflows/nightly-jobs.yml` (1-33).
8. `.github/actions/*/action.yml` (`build-docker`, `build-release`, `checkout-with-retry`, `run-command`, `run-docker`) — job execution infra.
9. `.bazelrc` and `.pre-commit-config.yaml` — project tool/lint defaults.
10. `install.sh` (lines 1-319) — CLI install flow; hardcoded version fallback and help text.
11. `VERSION` (line 1) — package version source.
12. `README.md` and docs files: `docs/src/developer/build.md`, `docs/src/start/installation.md`, `docs/src/start/full-compilation.md`, `docs/src/reference/runtime-flags.md`, `docs/src/troubleshoot/cosim-issues.md`.
13. `tests` directory layout (`find tests -maxdepth 3 -type d`) — test suite topology.

## Key Code
- Legacy toolchain knobs:
  - `VARS.bzl:3-6` include both `XILINX_TOOL_VERSION` and `XILINX_TOOL_LEGACY_VERSION = "2022.2"`.
  - `bazel/dependencies.bzl:16-17,239-253` load legacy vars and construct/use `xsim_legacy_rdi` via `vivado_legacy_path + XILINX_TOOL_LEGACY_VERSION + "/data/xsim"`.
  - `bazel/BUILD.bazel:54-59` defines `xsc_legacy_rdi` wrapper target.
  - `bazel/dpi_rules.bzl:152-168` defines `_dpi_legacy_rdi_library` and `dpi_legacy_rdi_library`.
  - `MODULE.bazel:161` imports legacy repo `xsim_legacy_rdi`.
- Source-of-truth drift points:
  - `VERSION:1` = `0.1.20260721`.
  - `install.sh:27-29,61-73` and docs `README.md:33-34,54`, `docs/src/start/installation.md:30-49` still reference `0.1.20260319`.
  - Docs still reference legacy tool references and versions: `docs/src/developer/build.md:73-88`, `docs/src/troubleshoot/cosim-issues.md:40`, `docs/src/reference/runtime-flags.md:17`.
- CI matrix/version mismatches:
  - `staging-build.yml:54-66` includes `2023.2`, `2023.1`, `2022.2`, `2022.1` even if new chain is 2024.2-only.
  - `staging-build.yml:138` still uses `actions/download-artifact@v3` while most jobs use v4.

## Architecture
- `VARS.bzl` + `bazel/vars.bzl` provide version/path values as Bazel vars.
- `MODULE.bazel` enables `//bazel:dependencies.bzl` extension (`load_dependencies`) and registers repos including `vitis_hls`, `xsim_xv`, and optionally legacy `xsim_legacy_rdi`.
- `bazel/dependencies.bzl` maps tool versions to concrete repo paths (legacy and current), using `xsim_legacy_rdi` for legacy Vivado/Vitis HLS layout.
- `bazel/BUILD.bazel` exports wrappers (`xsc_xv` and `xsc_legacy_rdi`) and toolenv setup used by compile/cosim flows.
- `bazel/dpi_rules.bzl` exposes DPI rule variants keyed to those wrappers.
- Workflows drive build/install/test through composite actions under `.github/actions/`.

## Start Here
Open `VARS.bzl` → `bazel/dependencies.bzl` → `MODULE.bazel` first to see all legacy toolchain touchpoints. Then inspect `staging-build.yml` for CI version coverage.

## Findings

### 1) Legacy tool-version support to remove (new no-backward-compat toolchain)
- `VARS.bzl:6` (`XILINX_TOOL_LEGACY_VERSION = "2022.2"`) — **high**
- `bazel/dependencies.bzl:16-17` loads legacy vars for dependency repo resolution — **high**
- `bazel/dependencies.bzl:239-253` hardcodes legacy XSIM path and defines `xsim_legacy_rdi` — **high**
- `MODULE.bazel:161` includes `xsim_legacy_rdi` in `use_repo(...)` — **high**
- `bazel/BUILD.bazel:54-59` defines `xsc_legacy_rdi` wrapper bound to legacy vars — **high**
- `bazel/dpi_rules.bzl:152-168` defines legacy DPI variants (`_dpi_legacy_rdi_library`, `dpi_legacy_rdi_library`) — **medium** (no active usage found)

### 2) Single-source-of-truth violations
- `VERSION:1` vs `install.sh:27-29,61-73` default string `0.1.20260319` — **high**
- `README.md:33-34,47-49` and `docs/src/start/installation.md:30-49` hardcode `0.1.20260319` while `VERSION` is `0.1.20260721` — **medium**
- Legacy version/path duplication in docs: `docs/src/developer/build.md:73-75`, `docs/src/start/installation.md:12`, `docs/src/start/full-compilation.md:14` mention old minimums/legacy paths while active defaults/CI still include `2024.2` — **medium**

### 3) Dead/disabled CI jobs, obsolete flows, and stale infra
- `staging-build.yml:64,66` still schedules legacy Vitis versions (`2022.2`, `2022.1`) contrary to legacy-drop target — **high**
- `staging-build.yml:138` uses `actions/download-artifact@v3` while build/release jobs use v4 (`39`, `46`, `112-113`) — **medium**
- `docs/src/reference/runtime-flags.md:17` documents `FRT_XSIM_LEGACY` for old Vivado command formats — **medium** (likely obsolete after removal)

### 4) Obsolete/legacy content in `tools/`
- `tools/` contains no standalone scripts or orchestration files; only `test-tools` cargo crate is present and used in tests (`tools/BUILD.bazel`, `bazel/test_tool_rules.bzl`, `tests/*/BUILD.bazel`) — **low**

### 5) Tests layout (for migration planning)
- Suites are in `tests/apps`, `tests/functional`, `tests/regression`, `tests/quality`.
- Bazel test targets are not uniformly present across all suites; notable BUILD files in `tests/apps`, `tests/functional/{reproducibility,report,shared-mmap}` and targeted test packages.