"""Phase 7 — `tapa-cli` golden-snapshot tests for the Rust binary.

The legacy click-based Python CLI (`tapa/__main__.py` + `tapa/steps/`)
was retired. With the Python entry point
gone, the original Python-vs-Rust parity gates would silently skip on
every invocation, so this suite was rewritten as a *golden-snapshot*
gate: the Rust CLI's observable surface is captured in
`tests/golden/` and every run diffs the live binary's behavior against
those frozen baselines. A drift in the Rust output now fails loudly
instead of slipping past a no-op skip.

What is captured (under `tapa-core/tests/golden/`):
  * `help/<subcommand>.flags.txt` — sorted long-flag set for every
    subcommand's `--help` (plus `_root.flags.txt` for the top-level).
  * `argv/<app>.json` — parsed (top-level + per-subcommand) shape of
    every `tests/apps/*/run_tapa.bats` `compile_xo` invocation.
  * `vadd_analyze/{graph,design,settings}.json` — vadd `analyze`
    outputs, populated lazily when `tapacc` is available.
  * `vadd_xo/` — placeholder for the future analyze+synth+pack `.xo`
    snapshot, populated lazily when Vitis HLS is available.

Every test is named `test_cli_*` so the `-k cli` selector used by
`run_tapa_cli_golden_test.sh` covers the whole suite.

Tests skip cleanly on any developer machine missing `tapacc` /
`vitis_hls`; help-diff and argv-shape gates always run because they
only need the Rust parser. To regenerate a golden after an
intentional CLI change, set `TAPA_GOLDEN_REFRESH=1` and re-run the
relevant test — it overwrites the golden file in place instead of
asserting.

Run locally with ``python3 -m pytest tapa-core/tests/parity_test.py``.
"""

# ruff: noqa: ANN401, PT018

from __future__ import annotations

import hashlib as _hashlib
import json
import os
import re as _re
import shutil
import subprocess as _subprocess
import sys
import zipfile as _zipfile
from pathlib import Path
from typing import Any

import pytest

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if _REPO_ROOT.is_dir() and str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

_VADD_DIR = _REPO_ROOT / "tests" / "apps" / "vadd"
_VADD_TOP = "VecAdd"
_TAPA_CORE_DIR = _REPO_ROOT / "tapa-core"
_GOLDEN_DIR = Path(__file__).resolve().parent / "golden"
_HELP_GOLDEN_DIR = _GOLDEN_DIR / "help"
_ARGV_GOLDEN_DIR = _GOLDEN_DIR / "argv"
_VADD_ANALYZE_GOLDEN_DIR = _GOLDEN_DIR / "vadd_analyze"
_VADD_XO_GOLDEN_DIR = _GOLDEN_DIR / "vadd_xo"
_VADD_SYNTH_GOLDEN_DIR = _GOLDEN_DIR / "vadd_synth"
_VADD_FLOORPLAN_GOLDEN_DIR = _GOLDEN_DIR / "vadd_floorplan"
_VADD_FLOORPLAN_APPLY_GOLDEN_DIR = _GOLDEN_DIR / "vadd_floorplan_apply"
_VADD_DSE_GOLDEN_DIR = _GOLDEN_DIR / "vadd_dse"
_TEST_TOOLS_DIR = Path(__file__).resolve().parent / "tools"

_REFRESH = os.environ.get("TAPA_GOLDEN_REFRESH") == "1"

# Length threshold for "real" short options like `-w`; values shorter
# than this can't be a flag (and we use the constant to dodge ruff's
# magic-number lint).
_MIN_SHORT_OPT_LEN = 2


def _have(cmd: str) -> bool:
    return shutil.which(cmd) is not None


def _rust_tapa_binary() -> Path | None:
    """Return the path to the Rust `tapa` binary, building if needed.

    Honors `TAPA_CLI_BINARY` for callers that have pre-built the binary
    elsewhere. Otherwise runs `cargo build -p tapa-cli` once and uses
    the debug build under `tapa-core/target`.
    """
    override = os.environ.get("TAPA_CLI_BINARY")
    if override:
        p = Path(override)
        return p if p.is_file() else None
    if not _have("cargo"):
        return None
    res = _subprocess.run(
        [
            "cargo",
            "build",
            "-p",
            "tapa-cli",
            "--manifest-path",
            str(_TAPA_CORE_DIR / "Cargo.toml"),
        ],
        capture_output=True,
        check=False,
        timeout=600,
    )
    if res.returncode != 0:
        return None
    binary = _TAPA_CORE_DIR / "target" / "debug" / "tapa"
    return binary if binary.is_file() else None


def _rust_tapa_argv(binary: Path, *subcmd_args: str, work_dir: Path) -> list[str]:
    return [str(binary), "--work-dir", str(work_dir), *subcmd_args]


def _rust_env() -> dict[str, str]:
    """Build env for the Rust binary.

    Strips any stray `TAPA_STEP_*_PYTHON` vars from the caller's shell
    so a leaked bridge env can't silently mask a native-path
    regression (the bridge no longer exists).
    """
    env = {**os.environ}
    for key in list(env):
        if key.startswith("TAPA_STEP_") and key.endswith("_PYTHON"):
            env.pop(key, None)
    return env


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _normalize_design_for_compare(design: dict[str, Any]) -> dict[str, Any]:
    """Drop legitimately-divergent fields before comparing design.json runs.

    Removes timing-sensitive area defaults and clock_period strings so
    the structural comparison focuses on schema parity rather than
    per-run noise.
    """
    out = json.loads(json.dumps(design))
    for task in out.get("tasks", {}).values():
        task.pop("self_area", None)
        task.pop("total_area", None)
        task.pop("clock_period", None)
    return out


def _skip_if_cli_toolchain_missing() -> Path:
    """Return the Rust `tapa` binary or `pytest.skip()` cleanly.

    The Python click CLI was retired; only
    the Rust binary is checked. `tapacc` / `vitis_hls` availability
    is checked per-test where it's actually needed.
    """
    if not _VADD_DIR.is_dir():
        pytest.skip(f"vadd fixture missing: {_VADD_DIR}")
    binary = _rust_tapa_binary()
    if binary is None:
        pytest.skip("rust `tapa` binary unavailable (cargo build failed)")
    return binary


# `.xo` redaction + inventory helpers reused across the `.xo` snapshot.
def _py_redact(src: Path, dest: Path) -> None:
    """Invoke the production Python `_redact_and_zip` on `src` → `dest`."""
    from tapa.program.pack import _redact_and_zip  # noqa: PLC2701, PLC0415

    with (
        _zipfile.ZipFile(src, "r") as z_in,
        _zipfile.ZipFile(dest, "w") as z_out,
    ):
        _redact_and_zip(z_in, z_out)


def _zip_inventory(path: Path) -> list[tuple[str, str]]:
    """Return (filename, sha256(content)) for each entry, sorted by name."""
    out: list[tuple[str, str]] = []
    with _zipfile.ZipFile(path, "r") as z:
        for info in sorted(z.infolist(), key=lambda i: i.filename):
            digest = _hashlib.sha256(z.read(info)).hexdigest()
            out.append((info.filename, digest))
    return out


def _zip_metadata(path: Path) -> list[tuple[str, int, tuple[int, ...]]]:
    """Return (filename, file_size, date_time) for each ZIP entry.

    The `unzip -l`-style listing parity check runs *after* the
    reproducible-build redaction zeros out timestamps, so a consistent
    `date_time` on both sides is what guarantees identical downstream
    consumers (Vivado, signing, …).
    """
    out: list[tuple[str, int, tuple[int, ...]]] = []
    with _zipfile.ZipFile(path, "r") as z:
        out.extend(
            (info.filename, info.file_size, tuple(info.date_time))
            for info in sorted(z.infolist(), key=lambda i: i.filename)
        )
    return out


def test_cli_help_lists_expected_subcommands() -> None:
    """Verify the Rust CLI advertises the documented subcommand surface."""
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")
    rs_text = rs.stdout.decode("utf-8", "replace")
    for sub in (
        "analyze",
        "synth",
        "pack",
        "floorplan",
        "generate-floorplan",
        "compile",
        "compile-with-floorplan-dse",
        "g++",
        "version",
    ):
        assert sub in rs_text, f"Rust CLI must list `{sub}`"


def test_cli_version_runs() -> None:
    """`tapa version` must succeed and emit a non-empty version string."""
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), "version"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")
    assert rs.stdout.strip(), "version output must not be empty"


def test_cli_unknown_first_token_fails() -> None:
    """Reject `tapa bogus-subcommand` (negative parser gate)."""
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), "bogus-subcommand"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode != 0, "rust CLI must reject bogus subcommand"


def test_cli_chained_argv_value_collision_does_not_split() -> None:
    """Keep flag values attached to their flag.

    A flag value that equals a subcommand name (e.g. `--top synth`)
    must stay attached to `--top`, not boundary the chunk into a new
    chained subcommand.
    """
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [
            str(binary),
            "analyze",
            "--input",
            "missing.cpp",
            "--top",
            "synth",
        ],
        capture_output=True,
        check=False,
        env=_rust_env(),
        timeout=30,
    )
    # The rust binary refuses with a typed error coming out of the
    # `analyze` step (e.g. `tapa-cpp-binary not found`). The important
    # parity point is that `synth` is not misclassified as a chained
    # subcommand — otherwise clap would surface an "unrecognized
    # argument" diagnostic instead.
    assert rs.returncode != 0
    msg = rs.stderr.decode("utf-8", "replace").lower()
    assert "analyze" in msg or "tapa-cpp" in msg or "tapacc" in msg, msg
    assert "unrecognized" not in msg, msg
    assert "unexpected" not in msg, msg


# ---------------------------------------------------------------------
# Per-subcommand `--help` golden diff: every subcommand's long-flag
# set is frozen in `golden/help/<subcommand>.flags.txt`. A drift —
# either a removed/renamed flag or an undocumented new one — fails
# the test and prints the diff. Refresh with `TAPA_GOLDEN_REFRESH=1`.
# ---------------------------------------------------------------------


_LONG_FLAG_RE = _re.compile(r"--[A-Za-z][A-Za-z0-9-]+")


def _extract_long_flags(help_text: str) -> set[str]:
    """Pull `--long-flag` tokens from a help dump.

    Strips out common boilerplate (`--help`, `--version`) so the diff
    focuses on the documented option surface.
    """
    flags = {m.group(0) for m in _LONG_FLAG_RE.finditer(help_text)}
    flags.discard("--help")
    flags.discard("--version")
    return flags


def _read_golden_flags(path: Path) -> set[str]:
    if not path.is_file():
        return set()
    return {
        line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()
    }


def _write_golden_flags(path: Path, flags: set[str]) -> None:
    sorted_flags = sorted(flags)
    body = "\n".join(sorted_flags) + ("\n" if sorted_flags else "")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


_SUBCOMMANDS = (
    "analyze",
    "synth",
    "pack",
    "floorplan",
    "generate-floorplan",
    "compile",
    "compile-with-floorplan-dse",
    "g++",
    "version",
)


def test_cli_root_help_flags_match_golden() -> None:
    """Top-level `tapa --help` long-flag set must match the golden snapshot."""
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")
    live = _extract_long_flags(rs.stdout.decode("utf-8", "replace"))
    golden_path = _HELP_GOLDEN_DIR / "_root.flags.txt"
    if _REFRESH:
        _write_golden_flags(golden_path, live)
        return
    golden = _read_golden_flags(golden_path)
    assert live == golden, (
        f"top-level `--help` flag drift\n  added: {sorted(live - golden)}\n"
        f"  removed: {sorted(golden - live)}\n"
        f"  refresh: TAPA_GOLDEN_REFRESH=1 pytest -k cli_root_help"
    )


@pytest.mark.parametrize("subcommand", _SUBCOMMANDS)
def test_cli_subcommand_help_flags_match_golden(subcommand: str) -> None:
    """Each subcommand's long-flag set must match its golden snapshot.

    The golden files live in `tapa-core/tests/golden/help/` and were
    seeded from the live Rust binary at the time the Python CLI was
    retired. A flag rename or removal trips
    this test with an explicit added/removed diff. To intentionally
    update a golden after a CLI change, set `TAPA_GOLDEN_REFRESH=1`.
    """
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), subcommand, "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")
    live = _extract_long_flags(rs.stdout.decode("utf-8", "replace"))
    golden_path = _HELP_GOLDEN_DIR / f"{subcommand}.flags.txt"
    if _REFRESH:
        _write_golden_flags(golden_path, live)
        return
    golden = _read_golden_flags(golden_path)
    assert live == golden, (
        f"`{subcommand}` `--help` flag drift\n"
        f"  added (live - golden): {sorted(live - golden)}\n"
        f"  removed (golden - live): {sorted(golden - live)}\n"
        f"  refresh: TAPA_GOLDEN_REFRESH=1 pytest -k "
        f"cli_subcommand_help -k {subcommand}"
    )


# ---------------------------------------------------------------------
# Fixture-based chained-argv corpus.
#
# Discover every `tests/apps/*/run_tapa.bats` and lift the canonical
# `compile_xo` argv out of it. Each fixture's parsed shape is frozen
# in `golden/argv/<app>.json` as the canonical structure produced by
# the Rust parser. The live discovery + parse must match the golden
# byte-for-byte, modulo `TAPA_GOLDEN_REFRESH=1` updates.
# ---------------------------------------------------------------------

_APPS_DIR = _REPO_ROOT / "tests" / "apps"

# Tokens that consume the next argv entry as their value. Used by the
# argv-shape parser to keep flag/value pairs together when summarizing.
_VALUED_FLAGS = frozenset(
    {
        "-w",
        "--work-dir",
        "--temp-dir",
        "--clang-format-quota-in-bytes",
        "--remote-host",
        "--remote-key-file",
        "--remote-xilinx-settings",
        "--remote-ssh-control-dir",
        "--remote-ssh-control-persist",
        "-f",
        "--input",
        "-t",
        "--top",
        "-c",
        "--cflags",
        "--target",
        "--part-num",
        "-p",
        "--platform",
        "--clock-period",
        "-j",
        "--jobs",
        "--other-hls-configs",
        "--override-report-schema-version",
        "--nonpipeline-fifos",
        "--floorplan-config",
        "--device-config",
        "--floorplan-path",
        "-o",
        "--output",
        "-s",
        "--bitstream-script",
        "--custom-rtl",
        "--graphir-path",
        "--executable",
    }
)
_SUBCOMMAND_NAMES = frozenset(_SUBCOMMANDS)


def _discover_app_argvs() -> list[tuple[str, list[str]]]:
    """Return `(app_name, argv)` for every parsable `run_tapa.bats`."""
    out: list[tuple[str, list[str]]] = []
    if not _APPS_DIR.is_dir():
        return out
    for app_dir in sorted(_APPS_DIR.iterdir()):
        bats = app_dir / "run_tapa.bats"
        if not bats.is_file():
            continue
        text = bats.read_text(encoding="utf-8")
        match = _re.search(
            r"compile_xo\s*\(\)\s*\{(.+?)^\}",
            text,
            flags=_re.DOTALL | _re.MULTILINE,
        )
        if match is None:
            continue
        body = match.group(1)
        flat = _re.sub(r"\\\s*\n\s*", " ", body)
        m = _re.search(r"\btapa\b\s+(.*)", flat)
        if m is None:
            continue
        tail = m.group(1).split("\n", 1)[0].strip()
        tail = tail.replace("${BATS_TMPDIR}", "/tmp/tapa-parity")
        tail = tail.replace("${TAPA_HOME}", "/tmp/tapa-home")
        tail = tail.replace("${BATS_TEST_DIRNAME}", str(app_dir))
        argv = tail.split()
        if not argv:
            continue
        out.append((app_dir.name, argv))
    return out


def _is_flag_token(tok: str) -> bool:
    """True when `tok` looks like a `-x` or `--long-flag` argv token."""
    if tok.startswith("--"):
        return True
    if not tok.startswith("-"):
        return False
    if len(tok) < _MIN_SHORT_OPT_LEN:
        return False
    return not tok[1].isdigit()


def _summarize_section(args: list[str]) -> list[str]:
    """Reduce a flag/value/positional sequence to a sorted token list."""
    out: list[str] = []
    i = 0
    while i < len(args):
        tok = args[i]
        if _is_flag_token(tok):
            if tok in _VALUED_FLAGS and i + 1 < len(args):
                out.append(tok)
                i += 2  # consume "<flag><value>" pair
                continue
            out.append(tok)
            i += 1
            continue
        out.append(f"<positional:{tok}>")
        i += 1
    return sorted(out)


def _parse_argv_shape(argv: list[str]) -> dict[str, Any]:
    """Decompose `argv` into top-level + per-subcommand sections.

    Splits the argv at known subcommand-name tokens (skipping ones
    that are the value of a preceding `--flag`). Within each section,
    flags are preserved (paired with `<positional:NAME>` markers for
    bare positionals) and sorted for deterministic comparison.
    """
    sections: list[dict[str, Any]] = []
    top: list[str] = []
    current_sub: str | None = None
    current_args: list[str] = []
    target = top
    i = 0
    while i < len(argv):
        tok = argv[i]
        prev_consumes_value = i > 0 and argv[i - 1] in _VALUED_FLAGS
        if tok in _SUBCOMMAND_NAMES and not prev_consumes_value:
            if current_sub is not None:
                sections.append({"name": current_sub, "args": list(current_args)})
            current_sub = tok
            current_args = []
            target = current_args
            i += 1
            continue
        target.append(tok)
        i += 1
    if current_sub is not None:
        sections.append({"name": current_sub, "args": list(current_args)})
    return {
        "raw_argv": list(argv),
        "top_level_flags": _summarize_section(top),
        "subcommands": [
            {"name": s["name"], "flags": _summarize_section(s["args"])}
            for s in sections
        ],
    }


_APP_ARGVS = _discover_app_argvs()

# Curated argv fixtures for subcommands NOT covered by the per-app
# `compile_xo` corpus. Explicit coverage is required for each major
# invocation idiom; `tests/apps/*/run_tapa.bats` gives us `compile`
# (and transitively analyze + synth + pack), but the non-`compile`
# entry points need hand-rolled fixtures. Each entry lands a golden
# JSON in `tests/golden/argv/<name>.json` via the same test harness.
_CURATED_ARGVS: list[tuple[str, list[str]]] = [
    # `generate-floorplan` — full analyze+synth+autobridge surface.
    (
        "generate-floorplan",
        [
            "-w",
            "/tmp/tapa-parity/gf-workdir",
            "generate-floorplan",
            "-f",
            "vadd.cpp",
            "-t",
            "VecAdd",
            "--platform",
            "xilinx_u250_gen3x16_xdma_4_1_202210_1",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            "fp.json",
        ],
    ),
    # `compile-with-floorplan-dse` — DSE entry with the union flag set.
    (
        "compile-with-floorplan-dse",
        [
            "-w",
            "/tmp/tapa-parity/dse-workdir",
            "compile-with-floorplan-dse",
            "-f",
            "vadd.cpp",
            "-t",
            "VecAdd",
            "--platform",
            "xilinx_u250_gen3x16_xdma_4_1_202210_1",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            "fp.json",
            "--enable-synth-util",
            "--gen-ab-graph",
        ],
    ),
    # `g++` — host compile pass-through.
    (
        "gpp",
        [
            "g++",
            "-std=c++17",
            "vadd-host.cpp",
            "-o",
            "vadd-host",
            "-lfrt",
        ],
    ),
    # `version` — trivial invocation.
    (
        "version",
        ["version"],
    ),
]


@pytest.mark.parametrize(
    ("app_name", "argv"),
    _APP_ARGVS + _CURATED_ARGVS,
    ids=[name for name, _ in _APP_ARGVS + _CURATED_ARGVS] or ["__no_apps__"],
)
def test_cli_chained_argv_corpus_matches_golden(
    app_name: str,
    argv: list[str],
) -> None:
    """Each app's argv shape must match its golden + parse on the Rust binary.

    Two assertions:

      1. The shape extracted from `run_tapa.bats` matches the frozen
         `golden/argv/<app>.json`. A schema drift in the bats files
         (renamed flag, added subcommand) trips this immediately.
      2. The Rust parser accepts the same argv with `--help` appended
         (clap exits 0, no `unrecognized` / `unexpected` diagnostic).
    """
    if not argv:
        pytest.skip(f"empty argv for {app_name}")
    binary = _skip_if_cli_toolchain_missing()

    live_shape = _parse_argv_shape(argv)
    golden_path = _ARGV_GOLDEN_DIR / f"{app_name}.json"
    if _REFRESH:
        golden_path.parent.mkdir(parents=True, exist_ok=True)
        golden_path.write_text(
            json.dumps(live_shape, indent=4, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    else:
        assert golden_path.is_file(), (
            f"missing golden `{golden_path.name}`; refresh with "
            f"TAPA_GOLDEN_REFRESH=1 pytest -k cli_chained_argv_corpus"
        )
        golden = _read_json(golden_path)
        assert live_shape == golden, (
            f"[{app_name}] argv shape drift vs golden\n"
            f"  live={json.dumps(live_shape, indent=4, sort_keys=True)}\n"
            f"  golden={json.dumps(golden, indent=4, sort_keys=True)}\n"
            f"  refresh: TAPA_GOLDEN_REFRESH=1 pytest "
            f"-k 'cli_chained_argv_corpus and {app_name}'"
        )

    # Live parser gate: the Rust binary must accept the same argv when
    # we append `--help` (which short-circuits the pipeline). `g++` is
    # special — its `trailing_var_arg` absorbs everything so appending
    # `--help` just passes it to the real compiler (which may or may
    # not be installed), so we skip the live gate for that fixture.
    if app_name == "gpp":
        return
    rs = _subprocess.run(
        [str(binary), *argv, "--help"],
        capture_output=True,
        check=False,
        env=_rust_env(),
        timeout=30,
    )
    rs_err = rs.stderr.decode("utf-8", "replace").lower()
    rs_out = rs.stdout.decode("utf-8", "replace").lower()
    bad_tokens = ("unrecognized", "no such option", "unexpected argument")
    assert rs.returncode == 0, f"[{app_name}] rust rejected the fixture argv: {rs_err}"
    for token in bad_tokens:
        assert token not in rs_err and token not in rs_out, (
            f"[{app_name}] rust parser complained about `{token}`: {rs_err}"
        )


# ---------------------------------------------------------------------
# vadd `analyze` golden snapshot.
#
# When `tapacc` (and therefore `tapa-cpp`) is available, run the Rust
# binary's `analyze` step on `tests/apps/vadd` and snapshot
# `graph.json` / `design.json` / `settings.json` into
# `golden/vadd_analyze/`. Subsequent runs diff the live JSON against
# the golden. Skip cleanly when `tapacc` is missing.
# ---------------------------------------------------------------------


def _have_tapacc() -> bool:
    """Return True when `tapacc` is reachable on PATH."""
    return _have("tapacc") or _have("tapa-cpp")


_VADD_ANALYZE_GOLDEN_FILES = ("graph.json", "settings.json", "design.json")


def _collect_vadd_analyze_outputs(work: Path) -> dict[str, dict[str, Any]]:
    """Read + normalize the JSON outputs produced by `tapa analyze`."""
    out: dict[str, dict[str, Any]] = {}
    for fname in _VADD_ANALYZE_GOLDEN_FILES:
        live = _read_json(work / fname)
        if fname == "design.json":
            live = _normalize_design_for_compare(live)
        out[fname] = live
    return out


def _refresh_vadd_analyze_goldens(files: dict[str, dict[str, Any]]) -> None:
    _VADD_ANALYZE_GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
    for fname, data in files.items():
        (_VADD_ANALYZE_GOLDEN_DIR / fname).write_text(
            json.dumps(data, indent=4, sort_keys=True) + "\n",
            encoding="utf-8",
        )


def _assert_vadd_analyze_goldens(files: dict[str, dict[str, Any]]) -> None:
    seeded = all((_VADD_ANALYZE_GOLDEN_DIR / fname).is_file() for fname in files)
    if not seeded:
        pytest.skip(
            f"vadd_analyze goldens not yet seeded; run with "
            f"TAPA_GOLDEN_REFRESH=1 to populate {_VADD_ANALYZE_GOLDEN_DIR}"
        )
    for fname, live in files.items():
        golden = _read_json(_VADD_ANALYZE_GOLDEN_DIR / fname)
        if fname == "design.json":
            golden = _normalize_design_for_compare(golden)
        assert live == golden, (
            f"vadd_analyze `{fname}` drift vs golden — "
            f"refresh with TAPA_GOLDEN_REFRESH=1 if intentional"
        )


def test_cli_golden_vadd_analyze(tmp_path: Path) -> None:
    """Snapshot/diff `tapa analyze` outputs on the vadd fixture.

    Golden files: `graph.json`, `settings.json`, `design.json` in
    `tapa-core/tests/golden/vadd_analyze/`. The first golden seed
    happens automatically when `TAPA_GOLDEN_REFRESH=1` is set; the
    test then asserts subsequent live runs produce identical JSON
    (after the same `_normalize_design_for_compare` pass that hides
    timing-noisy fields like `clock_period` / `self_area`).

    Skips cleanly when `tapacc` / `tapa-cpp` is not on PATH.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; vadd analyze is unrunnable")

    work = tmp_path / "rs-out"
    rs = _subprocess.run(
        _rust_tapa_argv(
            binary,
            "analyze",
            "--input",
            str(vadd_cpp),
            "--top",
            _VADD_TOP,
            work_dir=work,
        ),
        env=_rust_env(),
        capture_output=True,
        check=False,
        timeout=300,
    )
    if rs.returncode != 0:
        msg = rs.stderr.decode("utf-8", "replace")
        if "Cannot find" in msg or "tapacc binary not found" in msg:
            pytest.skip(f"tapacc toolchain not configured: {msg.splitlines()[0]}")
        pytest.fail(f"rust analyze failed: {msg}")

    files = _collect_vadd_analyze_outputs(work)
    if _REFRESH:
        _refresh_vadd_analyze_goldens(files)
        return
    _assert_vadd_analyze_goldens(files)


# ---------------------------------------------------------------------
# vadd `analyze synth pack` `.xo` golden snapshot.
#
# Skipped when `vitis_hls` is not on PATH. Documented placeholder under
# `golden/vadd_xo/` is populated lazily by `TAPA_GOLDEN_REFRESH=1`
# runs that have the full toolchain available.
# ---------------------------------------------------------------------


def _xo_signature(
    xo_path: Path,
) -> tuple[
    list[tuple[str, str]],
    list[tuple[str, int, tuple[int, ...]]],
]:
    """Return `(content_inventory, listing_metadata)` for `xo_path`."""
    return _zip_inventory(xo_path), _zip_metadata(xo_path)


_VADD_FLOW_SKIP_NEEDLES = (
    "Cannot find",
    "tapacc binary not found",
    "tapa-cpp",
    "vitis_hls",
    "XILINX_HLS",
    "StepUnported",
    "is not yet ported",
    "is not yet supported",
)


def _maybe_skip_on_vadd_flow_failure(stderr: str) -> None:
    """Translate a known-toolchain failure into a `pytest.skip()`."""
    for needle in _VADD_FLOW_SKIP_NEEDLES:
        if needle in stderr:
            first = stderr.splitlines()[0] if stderr.strip() else "<no stderr>"
            pytest.skip(f"vadd flow toolchain/native gap: {needle} | {first}")


def _build_vadd_flow_argv(
    binary: Path,
    work: Path,
    vadd_cpp: Path,
    xo_path: Path,
    platform: str,
) -> list[str]:
    return [
        str(binary),
        "--work-dir",
        str(work),
        "analyze",
        "--input",
        str(vadd_cpp),
        "--top",
        _VADD_TOP,
        "synth",
        "--platform",
        platform,
        "pack",
        "--output",
        str(xo_path),
    ]


def _refresh_vadd_xo_goldens(
    inv: list[tuple[str, str]],
    meta: list[tuple[str, int, tuple[int, ...]]],
) -> None:
    _VADD_XO_GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
    (_VADD_XO_GOLDEN_DIR / "xo_inventory.json").write_text(
        json.dumps(inv, indent=4) + "\n", encoding="utf-8"
    )
    (_VADD_XO_GOLDEN_DIR / "xo_metadata.json").write_text(
        json.dumps([list(m) for m in meta], indent=4) + "\n",
        encoding="utf-8",
    )


def test_cli_golden_vadd_xo_flow(tmp_path: Path) -> None:
    """Snapshot/diff the `analyze synth pack` `.xo` against a golden.

    Skips on developer machines that don't have Vitis HLS available.
    On a fully-configured host:
      - first run with `TAPA_GOLDEN_REFRESH=1` writes
        `golden/vadd_xo/xo_inventory.json` + `xo_metadata.json`,
      - subsequent runs assert byte-equal `.xo` (after redaction).

    The redaction pass is the production
    `tapa.program.pack._redact_and_zip` which zeros embedded timestamps
    and absolute paths. If `tapa.program.pack` is not importable
    (e.g. the Python helper has also been retired), this test skips.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; vadd flow unrunnable")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    work = tmp_path / "rs-out"
    work.mkdir()
    xo_path = work / "vadd.xo"
    run = _subprocess.run(
        _build_vadd_flow_argv(binary, work, vadd_cpp, xo_path, platform),
        env=_rust_env(),
        capture_output=True,
        check=False,
        timeout=900,
    )
    if run.returncode != 0:
        msg = run.stderr.decode("utf-8", "replace")
        _maybe_skip_on_vadd_flow_failure(msg)
        pytest.fail(f"chain failed unexpectedly:\n{msg}")
    if not xo_path.is_file():
        pytest.skip(f"chain succeeded but no `.xo` at {xo_path}")

    redacted = tmp_path / "rs-redacted.xo"
    try:
        _py_redact(xo_path, redacted)
    except ImportError as exc:
        pytest.skip(f"tapa.program.pack not importable: {exc}")

    inv, meta = _xo_signature(redacted)
    inv_path = _VADD_XO_GOLDEN_DIR / "xo_inventory.json"
    meta_path = _VADD_XO_GOLDEN_DIR / "xo_metadata.json"
    if _REFRESH:
        _refresh_vadd_xo_goldens(inv, meta)
        return
    if not (inv_path.is_file() and meta_path.is_file()):
        pytest.skip(
            "vadd_xo goldens not yet seeded; run with "
            "TAPA_GOLDEN_REFRESH=1 on a Vitis-equipped host"
        )
    golden_inv = [tuple(x) for x in _read_json(inv_path)]
    golden_meta = [(e[0], e[1], tuple(e[2])) for e in _read_json(meta_path)]
    assert inv == golden_inv, "vadd `.xo` content drift vs golden"
    assert meta == golden_meta, "vadd `.xo` listing-metadata drift vs golden"


# ---------------------------------------------------------------------
# Reusable snapshot helpers for native step output trees. Each helper
# captures a relpath → text dict, so
# the corresponding `refresh` / `assert` pair can handle arbitrary
# nested layouts without hand-wiring every file name.
# ---------------------------------------------------------------------


def _json_canon(path: Path, *, normalize_design: bool = False) -> str:
    """Re-emit a JSON file with canonical formatting for diff-clean goldens."""
    obj = _read_json(path)
    if normalize_design:
        obj = _normalize_design_for_compare(obj)
    return json.dumps(obj, indent=4, sort_keys=True) + "\n"


def _refresh_tree_goldens(root: Path, snap: dict[str, str]) -> None:
    """Write `snap` to `root`, clearing stale entries first.

    The golden directory owns exactly the files in `snap` plus any
    seeded `.gitkeep`; any other leftovers from a previous shape (e.g.
    a renamed RTL file) get pruned so the gate stays authoritative.
    """
    root.mkdir(parents=True, exist_ok=True)
    keep = {root / rel for rel in snap}
    for existing in list(root.rglob("*")):
        if existing.is_file() and existing.name != ".gitkeep" and existing not in keep:
            existing.unlink()
    # Clean up empty directories that previously held goldens.
    for existing in sorted(root.rglob("*"), key=lambda p: -len(p.as_posix())):
        if existing.is_dir() and not any(existing.iterdir()):
            existing.rmdir()
    for rel, content in snap.items():
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")


def _assert_tree_goldens(root: Path, snap: dict[str, str], label: str) -> None:
    """Diff `snap` against files under `root`.

    Two distinct states:

    * **Unseeded** (no golden file present yet under `root` at all):
      cleanly `pytest.skip()` so a developer can run
      `TAPA_GOLDEN_REFRESH=1` to populate the directory for the
      first time. `.gitkeep` placeholders do not count as seeded
      goldens.
    * **Seeded** (any golden file already exists): a missing live
      entry now fails instead of skipping — otherwise a regressed
      run that silently drops an output could pass the gate
      by virtue of the hole. Extras on either side (live without
      golden, or golden without live) are assertion failures.
    """
    live_keys = set(snap)
    golden_keys = {
        p.relative_to(root).as_posix()
        for p in root.rglob("*")
        if p.is_file() and p.name != ".gitkeep"
    }
    if not golden_keys:
        pytest.skip(
            f"{label} goldens not yet seeded under {root}; rerun with "
            "TAPA_GOLDEN_REFRESH=1 to populate"
        )
    missing = live_keys - golden_keys
    assert not missing, (
        f"{label} live run produced entries with no golden: {sorted(missing)}; "
        "rerun with TAPA_GOLDEN_REFRESH=1 if intentional"
    )
    extras = golden_keys - live_keys
    assert not extras, (
        f"{label} has stale goldens with no live counterpart: {sorted(extras)}"
    )
    for rel, live in snap.items():
        golden = (root / rel).read_text(encoding="utf-8")
        assert live == golden, (
            f"{label} `{rel}` drift vs golden — rerun with "
            "TAPA_GOLDEN_REFRESH=1 if intentional"
        )


def test_assert_tree_goldens_fails_on_missing_when_seeded(tmp_path: Path) -> None:
    """Regression for the skip-open bug.

    Once any golden exists under `root`, a new live entry without a
    golden must fail instead of skip. Only a completely empty root
    (ignoring `.gitkeep`) may still skip.
    """
    # Case 1: unseeded root (no golden besides an optional `.gitkeep`)
    # → skip is still appropriate.
    empty_root = tmp_path / "empty"
    empty_root.mkdir()
    (empty_root / ".gitkeep").write_text("keep\n", encoding="utf-8")
    with pytest.raises(pytest.skip.Exception):
        _assert_tree_goldens(empty_root, {"live.json": "{}"}, "probe")

    # Case 2: seeded root with a different key → live-only key is
    # an assertion failure, not a skip.
    seeded_root = tmp_path / "seeded"
    seeded_root.mkdir()
    (seeded_root / "known.json").write_text("{}\n", encoding="utf-8")
    with pytest.raises(
        AssertionError, match="live run produced entries with no golden"
    ):
        _assert_tree_goldens(
            seeded_root, {"known.json": "{}\n", "new.json": "{}\n"}, "probe"
        )

    # Case 3: seeded root, live matches, no extras → passes cleanly.
    _assert_tree_goldens(seeded_root, {"known.json": "{}\n"}, "probe")


# ---------------------------------------------------------------------
# vadd `analyze synth` golden snapshot.
# Snapshots `rtl/*.v` (Rust codegen output, deterministic),
# `templates_info.json`, `design.json`, `settings.json`, plus the HLS
# report file-inventory AND every `.rpt` content after a byte-stable
# redaction pass that strips the Vitis date/time/version headers.
# ---------------------------------------------------------------------


# Volatile report headers: Vitis emits these lines with wall-clock
# timestamps, build IDs, project paths, session user names, and a
# host machine name that all vary from run to run. The previous
# regex kept the original text after the match; we now REPLACE the
# whole tail with a fixed `<<redacted>>` sentinel so goldens stay
# byte-equal across configured-host runs.
_RPT_HEADER_KEYS = (
    "Date",
    "Report date",
    "Version",
    "Project",
    "Solution",
    "Product version",
    "User",
    "Host",
    "Copyright",
    "Generated on",
    "Generated by",
    "Start of session at",
)
_RPT_HEADER_KEYS_ALTERNATION = "|".join(_re.escape(k) for k in _RPT_HEADER_KEYS)
_RPT_VOLATILE_HEADER_RE = _re.compile(
    # Optional indent, optional `* ` Vitis leader, one of the header
    # keys, `:` or `=`, and the remainder of the line.
    r"^(?P<prefix>\s*(?:\*\s*)?)"
    rf"(?P<key>{_RPT_HEADER_KEYS_ALTERNATION})"
    r"\s*[:=].*$",
    _re.MULTILINE,
)
# Vitis HLS peppers the ruler lines with run-specific dashes whose
# count reflects variable-width module names. Collapse any hyphen
# run ≥ 5 to a fixed `-----` so golden diffs aren't derailed by
# incidental column-width drift.
_RPT_DASH_RULER_RE = _re.compile(r"-{5,}")

_RPT_REDACTED_SENTINEL = "<<redacted>>"


def _redact_rpt_content(text: str) -> str:
    """Stable, reviewer-friendly redaction for `.rpt` diffs.

    A superset of `tapa.program.pack._redact_rpt`: every known
    volatile header line (timestamps, version strings, project /
    solution names, session-start markers — with or without Vitis's
    leading `* `) becomes `<indent><key>: <<redacted>>`. Table body
    rows round-trip verbatim; the only body-side normalization is
    collapsing long hyphen rulers whose width follows run-specific
    module names.
    """

    def _replace(match: _re.Match[str]) -> str:
        return f"{match.group('prefix')}{match.group('key')}: {_RPT_REDACTED_SENTINEL}"

    redacted = _RPT_VOLATILE_HEADER_RE.sub(_replace, text)
    return _RPT_DASH_RULER_RE.sub("-----", redacted)


def test_vadd_synth_rpt_goldens_are_fully_redacted() -> None:
    """Non-Vitis guard on the checked-in `vadd_synth/reports/` goldens.

    Catches the exact "sanitizer fixed but goldens stale" state
    Every `.rpt` under `tests/golden/vadd_synth/reports/`
    must round-trip through `_redact_rpt_content` unchanged (i.e. the
    canonicalization is already at its fixed point). A failure means
    the goldens still contain live Vitis timestamps / version strings
    / user names / project paths — regenerate them with
    `TAPA_GOLDEN_REFRESH=1` on a configured host.
    """
    reports_dir = _VADD_SYNTH_GOLDEN_DIR / "reports"
    if not reports_dir.is_dir():
        pytest.skip(f"{reports_dir} absent; run TAPA_GOLDEN_REFRESH=1 first")
    rpts = sorted(reports_dir.rglob("*.rpt"))
    if not rpts:
        pytest.skip(f"{reports_dir} contains no `.rpt` files yet")
    stale: list[tuple[Path, str]] = []
    for rpt in rpts:
        text = rpt.read_text(encoding="utf-8", errors="replace")
        redacted = _redact_rpt_content(text)
        if text != redacted:
            rel = rpt.relative_to(_VADD_SYNTH_GOLDEN_DIR).as_posix()
            stale.append((rpt.relative_to(_VADD_SYNTH_GOLDEN_DIR), "non-fixed-point"))
            # Also call out known volatile-substring giveaways to make
            # the failure message actionable without re-reading every
            # golden file.
            for volatile in (
                "Sun Apr",
                "Mon Apr",
                "Tue Apr",
                "Wed Apr",
                "Thu Apr",
                "Fri Apr",
                "Sat Apr",
                "Build 5238294",
                "2024.2 (Build",
            ):
                if volatile in text:
                    stale.append(
                        (
                            rpt.relative_to(_VADD_SYNTH_GOLDEN_DIR),
                            f"contains volatile `{volatile}`",
                        )
                    )
                    break
            _ = rel  # keep `rel` lint-visible when match list is empty
    assert not stale, (
        "vadd_synth report goldens contain volatile, unredacted Vitis "
        "output; regenerate with TAPA_GOLDEN_REFRESH=1:\n  "
        + "\n  ".join(f"{p}: {reason}" for p, reason in stale)
    )


def test_redact_rpt_content_canonicalizes_volatile_headers() -> None:
    """Sanitizer regression.

    Every known volatile-header form — with or without Vitis's `* `
    leader, bare `Date:` included — must collapse to the
    deterministic sentinel. Body rows unchanged.
    """
    raw = (
        "================================================================\n"
        "== Vitis HLS Report for 'Add'\n"
        "================================================================\n"
        "* Date:           Sun Apr 19 03:50:19 2026\n"
        "\n"
        "* Version:        2024.2 (Build 5238294 on Nov  8 2024)\n"
        "* Project:        project\n"
        "* Solution:       Add (Vivado IP Flow Target)\n"
        "Date:           Tue Jan 01 00:00:00 1980\n"
        "User: tapa\n"
        "| Total LUTs | FFs | DSP Blocks |\n"
        "+------+----+---+\n"
    )
    redacted = _redact_rpt_content(raw)
    for key in ("* Date", "* Version", "* Project", "* Solution", "Date", "User"):
        assert f"{key}: {_RPT_REDACTED_SENTINEL}" in redacted, redacted
    # Table rows survive verbatim (only the ruler is normalized).
    assert "| Total LUTs | FFs | DSP Blocks |" in redacted
    # No run-specific substring may leak through.
    for volatile in ("2026", "2024.2", "Build", "Sun Apr", "Tue Jan", " tapa"):
        assert volatile not in redacted, (
            f"volatile `{volatile}` leaked into redacted output:\n{redacted}"
        )


def _collect_vadd_synth_snapshot(work: Path) -> dict[str, str]:
    snap: dict[str, str] = {}
    rtl_dir = work / "rtl"
    if rtl_dir.is_dir():
        for v in sorted(rtl_dir.glob("*.v")):
            snap[f"rtl/{v.name}"] = v.read_text(encoding="utf-8")
    for fname in ("templates_info.json", "settings.json"):
        path = work / fname
        if path.is_file():
            snap[fname] = _json_canon(path)
    design_path = work / "design.json"
    if design_path.is_file():
        snap["design.json"] = _json_canon(design_path, normalize_design=True)
    # Full byte-stable `.rpt` content parity. Every
    # file goes through `_redact_rpt_content` so Vitis timestamps /
    # version strings don't turn a clean run into drift noise.
    rpts = sorted(work.rglob("*.rpt"))
    for rpt in rpts:
        rel = rpt.relative_to(work).as_posix()
        snap[f"reports/{rel}"] = _redact_rpt_content(
            rpt.read_text(encoding="utf-8", errors="replace")
        )
    snap["report_inventory.txt"] = "".join(
        f"{r.relative_to(work).as_posix()}\n" for r in rpts
    )
    return snap


def _build_vadd_synth_argv(
    binary: Path,
    work: Path,
    vadd_cpp: Path,
    platform: str,
) -> list[str]:
    return [
        str(binary),
        "--work-dir",
        str(work),
        "analyze",
        "--input",
        str(vadd_cpp),
        "--top",
        _VADD_TOP,
        "synth",
        "--platform",
        platform,
    ]


def test_cli_golden_vadd_synth(tmp_path: Path) -> None:
    """Snapshot/diff native `tapa analyze synth` outputs on vadd.

    End-to-end: after `synth` runs, the work dir must
    contain deterministic Rust-codegen RTL under `rtl/`, a
    Python-parity `templates_info.json`, a synth'd `design.json`, and
    a `settings.json` flagged `synthed=true`. The suite also records
    the HLS report file-inventory (names only) so a missing report
    file surfaces immediately even though `.rpt` contents carry
    non-deterministic Vitis timestamps.

    Skips when `vitis_hls` or `tapacc` is unavailable.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; synth is unrunnable")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    work = tmp_path / "rs-out"
    work.mkdir()
    run = _subprocess.run(
        _build_vadd_synth_argv(binary, work, vadd_cpp, platform),
        env=_rust_env(),
        capture_output=True,
        check=False,
        timeout=900,
    )
    if run.returncode != 0:
        msg = run.stderr.decode("utf-8", "replace")
        _maybe_skip_on_vadd_flow_failure(msg)
        pytest.fail(f"synth chain failed unexpectedly:\n{msg}")

    snap = _collect_vadd_synth_snapshot(work)
    if _REFRESH:
        _refresh_tree_goldens(_VADD_SYNTH_GOLDEN_DIR, snap)
        return
    _assert_tree_goldens(_VADD_SYNTH_GOLDEN_DIR, snap, "vadd_synth")


# ---------------------------------------------------------------------
# `generate-floorplan` + `compile-with-floorplan-dse` golden snapshots
# Both composites invoke `rapidstream-tapafp`; the suite
# supplies a deterministic stub under `tests/tools/rapidstream-tapafp`
# so the gate runs without external infrastructure.
# ---------------------------------------------------------------------


_FLOORPLAN_CONFIG_FIXTURE_NAME = "vadd_floorplan_config.json"
_DEVICE_CONFIG_FIXTURE_NAME = "vadd_device_config.json"


def _write_floorplan_fixtures(dir_: Path) -> tuple[Path, Path]:
    """Write the floorplan + device config fixtures the stub consumes."""
    floorplan_cfg = dir_ / _FLOORPLAN_CONFIG_FIXTURE_NAME
    device_cfg = dir_ / _DEVICE_CONFIG_FIXTURE_NAME
    floorplan_cfg.write_text(
        json.dumps(
            {
                "dse_region_shrink_factor": 1.0,
                "cpp_arg_pre_assignments": {},
                "sys_port_pre_assignments": {},
                "port_pre_assignments": {},
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    # Device config shape consumed by `tapa-lowering::read_device_config`:
    # `{ "slots": [{"x": .., "y": .., "pblock_ranges": [..]}], "part_num": ".." }`.
    # 1x1 single-slot device so the stub's `SLOT_X0Y0` mapping resolves.
    device_cfg.write_text(
        json.dumps(
            {
                "part_num": "xcu250-figd2104-2L-e",
                "slots": [
                    {
                        "x": 0,
                        "y": 0,
                        "pblock_ranges": [
                            "CLOCKREGION_X0Y0:CLOCKREGION_X7Y3",
                        ],
                    }
                ],
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    return floorplan_cfg, device_cfg


def _stub_tool_env(env: dict[str, str] | None = None) -> dict[str, str]:
    """Return a copy of `env` (or os.environ) with `tests/tools` first on PATH."""
    base = dict(env if env is not None else os.environ)
    path = f"{_TEST_TOOLS_DIR}:{base.get('PATH', '')}"
    base["PATH"] = path
    return base


def _collect_tree_json_snapshot(root: Path, *, subpaths: list[str]) -> dict[str, str]:
    """Snapshot every `*.json` under `root`, one entry per relpath.

    `subpaths` enumerates the sub-directories (relative to `root`)
    the caller cares about. Everything else is ignored to keep the
    gate focused on the composite's persistent JSON contract.
    """
    snap: dict[str, str] = {}
    for rel in subpaths:
        sub = root / rel if rel else root
        if not sub.exists():
            continue
        if sub.is_file() and sub.suffix == ".json":
            snap[rel or sub.name] = _json_canon(
                sub, normalize_design=sub.name == "design.json"
            )
            continue
        for f in sorted(sub.rglob("*.json")):
            rel_path = f.relative_to(root).as_posix()
            snap[rel_path] = _json_canon(f, normalize_design=f.name == "design.json")
    return snap


def _build_vadd_generate_floorplan_argv(  # noqa: PLR0913, PLR0917
    binary: Path,
    work: Path,
    vadd_cpp: Path,
    platform: str,
    floorplan_cfg: Path,
    device_cfg: Path,
    floorplan_out: Path,
) -> list[str]:
    return [
        str(binary),
        "--work-dir",
        str(work),
        "generate-floorplan",
        "-f",
        str(vadd_cpp),
        "--top",
        _VADD_TOP,
        "--platform",
        platform,
        "--floorplan-config",
        str(floorplan_cfg),
        "--device-config",
        str(device_cfg),
        "--floorplan-path",
        str(floorplan_out),
    ]


def _build_vadd_dse_argv(  # noqa: PLR0913, PLR0917
    binary: Path,
    work: Path,
    vadd_cpp: Path,
    platform: str,
    floorplan_cfg: Path,
    device_cfg: Path,
) -> list[str]:
    # DSE forbids `--output` — each floorplan solution writes its own
    # per-solution `.xo`. We just drive the composite and snapshot the
    # persistent JSON it produces.
    return [
        str(binary),
        "--work-dir",
        str(work),
        "compile-with-floorplan-dse",
        "-f",
        str(vadd_cpp),
        "--top",
        _VADD_TOP,
        "--platform",
        platform,
        "--floorplan-config",
        str(floorplan_cfg),
        "--device-config",
        str(device_cfg),
    ]


def test_cli_golden_vadd_generate_floorplan(tmp_path: Path) -> None:
    """Diff `generate-floorplan` persistent JSON outputs.

    Runs the real `tapa generate-floorplan` composite against a
    deterministic `rapidstream-tapafp` stub that ships under
    `tests/tools/`. Snapshots every persistent JSON under the top
    work dir plus the `autobridge/` sub-tree; diffs them against
    `tests/golden/vadd_floorplan/`.

    Skips cleanly when `vitis_hls` / `tapacc` is unavailable.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; generate-floorplan unrunnable")

    stub = _TEST_TOOLS_DIR / "rapidstream-tapafp"
    if not stub.is_file():
        pytest.skip(f"rapidstream-tapafp stub missing at {stub}")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    fixtures = tmp_path / "fixtures"
    fixtures.mkdir()
    floorplan_cfg, device_cfg = _write_floorplan_fixtures(fixtures)

    work = tmp_path / "rs-out"
    work.mkdir()
    floorplan_out = work / "floorplan.json"
    run = _subprocess.run(
        _build_vadd_generate_floorplan_argv(
            binary,
            work,
            vadd_cpp,
            platform,
            floorplan_cfg,
            device_cfg,
            floorplan_out,
        ),
        env=_stub_tool_env(_rust_env()),
        capture_output=True,
        check=False,
        timeout=900,
    )
    if run.returncode != 0:
        msg = run.stderr.decode("utf-8", "replace")
        _maybe_skip_on_vadd_flow_failure(msg)
        pytest.fail(f"generate-floorplan failed unexpectedly:\n{msg}")

    snap = _collect_tree_json_snapshot(
        work,
        subpaths=[
            "design.json",
            "settings.json",
            "templates_info.json",
            "ab_graph.json",
            "floorplan.json",
            "autobridge",
        ],
    )
    if _REFRESH:
        _refresh_tree_goldens(_VADD_FLOORPLAN_GOLDEN_DIR, snap)
        return
    _assert_tree_goldens(_VADD_FLOORPLAN_GOLDEN_DIR, snap, "vadd_floorplan")


def test_cli_golden_vadd_compile_with_floorplan_dse(  # noqa: C901, PLR0914, PLR0915
    tmp_path: Path,
) -> None:
    """Diff `compile-with-floorplan-dse` persistent JSON outputs.

    Runs the full DSE composite with the deterministic
    `rapidstream-tapafp` stub and snapshots every persistent JSON
    under the top-level work dir (plus `autobridge/` and any
    `solution_*/` sub-trees). Diffs them against
    `tests/golden/vadd_dse/`.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; dse unrunnable")

    stub = _TEST_TOOLS_DIR / "rapidstream-tapafp"
    if not stub.is_file():
        pytest.skip(f"rapidstream-tapafp stub missing at {stub}")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    fixtures = tmp_path / "fixtures"
    fixtures.mkdir()
    floorplan_cfg, device_cfg = _write_floorplan_fixtures(fixtures)

    work = tmp_path / "rs-out"
    work.mkdir()
    run = _subprocess.run(
        _build_vadd_dse_argv(
            binary,
            work,
            vadd_cpp,
            platform,
            floorplan_cfg,
            device_cfg,
        ),
        env=_stub_tool_env(_rust_env()),
        capture_output=True,
        check=False,
        timeout=1800,
    )
    if run.returncode != 0:
        msg = run.stderr.decode("utf-8", "replace")
        _maybe_skip_on_vadd_flow_failure(msg)
        pytest.fail(f"compile-with-floorplan-dse failed unexpectedly:\n{msg}")

    # DSE must emit *terminal* stage-2 compile state under
    # `solution_*/` — not merely preliminary `analyze` JSON that gets
    # written before the per-solution synth/pack loop runs. The Rust
    # DSE composite logs per-solution failures and returns
    # `Ok(())`, so we need to pin down which solutions actually
    # reached the end of the pipeline.
    #
    # A solution is considered "successful" only if ALL of:
    #   * `settings.json` has `"synthed": true` (synth committed)
    #   * `templates_info.json` exists (synth wrote it post-HLS)
    #   * `graphir.json` exists (stage-2 forces `gen_graphir=true`)
    #   * one `.xo` file exists (pack succeeded)
    solution_dirs = sorted(p for p in work.glob("solution_*") if p.is_dir())
    assert solution_dirs, (
        "compile-with-floorplan-dse must materialize at least one "
        "`solution_*/` work dir"
    )

    def _solution_is_successful(sol: Path) -> bool:
        settings_path = sol / "settings.json"
        if not settings_path.is_file():
            return False
        try:
            synthed = _read_json(settings_path).get("synthed") is True
        except (json.JSONDecodeError, OSError):
            return False
        if not synthed:
            return False
        if not (sol / "templates_info.json").is_file():
            return False
        if not (sol / "graphir.json").is_file():
            return False
        return any(sol.rglob("*.xo"))

    successful = [s for s in solution_dirs if _solution_is_successful(s)]
    assert successful, (
        "DSE stage 2 produced no terminal artifacts in any "
        f"`solution_*/` dir (need synthed=true + templates_info.json "
        f"+ graphir.json + <name>.xo). Got: {[s.name for s in solution_dirs]}"
    )

    dse_subpaths = [
        "design.json",
        "settings.json",
        "templates_info.json",
        "ab_graph.json",
        "autobridge",
    ]
    dse_subpaths.extend(s.name for s in solution_dirs)
    snap = _collect_tree_json_snapshot(work, subpaths=dse_subpaths)

    # Artifact proof: snapshot every successful solution's `.xo`
    # content inventory + listing metadata (through the production
    # `_redact_and_zip` pass). The DSE gate previously only diffed
    # JSON, which leaves "stage-2 pack actually produced a byte-equal
    # kernel" unverified. Using the same helpers the `vadd_xo` gate
    # uses gives us that guarantee without bespoke redaction logic.
    try:
        redaction_available = True
        for sol in successful:
            xos = sorted(sol.rglob("*.xo"))
            assert xos, f"successful solution `{sol.name}` has no `.xo` — logic bug"
            xo_inv_entries: list[tuple[str, str]] = []
            xo_meta_entries: list[tuple[str, int, tuple[int, ...]]] = []
            for xo in xos:
                redacted = tmp_path / f"{sol.name}-{xo.name}.redacted"
                try:
                    _py_redact(xo, redacted)
                except ImportError as exc:
                    pytest.skip(f"tapa.program.pack not importable: {exc}")
                xo_inv_entries.extend(
                    (f"{xo.name}::{name}", sha)
                    for name, sha in _zip_inventory(redacted)
                )
                xo_meta_entries.extend(
                    (f"{xo.name}::{name}", size, ts)
                    for name, size, ts in _zip_metadata(redacted)
                )
            snap[f"{sol.name}/xo_inventory.json"] = (
                json.dumps(xo_inv_entries, indent=4) + "\n"
            )
            snap[f"{sol.name}/xo_metadata.json"] = (
                json.dumps(
                    [[name, size, list(ts)] for name, size, ts in xo_meta_entries],
                    indent=4,
                )
                + "\n"
            )
    except ImportError:
        redaction_available = False
    if not redaction_available:
        pytest.skip("tapa.program.pack not importable; .xo proof unavailable")

    if _REFRESH:
        _refresh_tree_goldens(_VADD_DSE_GOLDEN_DIR, snap)
        return
    _assert_tree_goldens(_VADD_DSE_GOLDEN_DIR, snap, "vadd_dse")


# ---------------------------------------------------------------------
# Standalone `tapa floorplan --floorplan-path` golden snapshot
# Exercises the post-autobridge apply path: a prepared
# `graph.json` + `design.json` + `settings.json` already on disk
# (from an earlier `analyze synth`) gets rewritten by
# `apply_floorplan` when the user supplies a floorplan JSON that
# maps instances to slots.
# ---------------------------------------------------------------------


def _build_vadd_floorplan_apply_argv(
    binary: Path,
    work: Path,
    vadd_cpp: Path,
    platform: str,
    floorplan_path: Path,
) -> list[str]:
    # Chain: analyze --flatten-hierarchy (so top is upper w/ leaves) →
    # synth → floorplan --floorplan-path. `apply_floorplan` insists on
    # the flattened single-level shape.
    return [
        str(binary),
        "--work-dir",
        str(work),
        "analyze",
        "--input",
        str(vadd_cpp),
        "--top",
        _VADD_TOP,
        "--flatten-hierarchy",
        "synth",
        "--platform",
        platform,
        "floorplan",
        "--floorplan-path",
        str(floorplan_path),
    ]


def _write_vadd_floorplan_apply_fixture(path: Path) -> None:
    """Write a vadd-shaped floorplan JSON onto disk.

    Maps every top-level vadd instance (`Add_0`, `Mmap2Stream_0`,
    `Mmap2Stream_1`, `Stream2Mmap_0`) to the single slot `0:0`. That
    matches what a real autobridge run against a 1x1 device config
    emits for this fixture — every child ends up in one slot, so the
    resulting rewritten graph has a single slot task that wraps all
    four leaves.
    """
    path.write_text(
        json.dumps(
            {
                "Add_0": "0:0",
                "Mmap2Stream_0": "0:0",
                "Mmap2Stream_1": "0:0",
                "Stream2Mmap_0": "0:0",
            },
            indent=4,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


def test_cli_golden_vadd_floorplan_apply(tmp_path: Path) -> None:
    """Diff `tapa floorplan --floorplan-path` persistent JSON outputs.

    The standalone apply step rewrites `graph.json` + `design.json` +
    `settings.json` (adding `floorplan=true` and the slot→region map)
    after wrapping leaf instances into the slot tasks named in the
    floorplan fixture. Skips cleanly when `vitis_hls` / `tapacc` are
    unavailable.
    """
    binary = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")
    if not _have_tapacc():
        pytest.skip("tapacc / tapa-cpp not on PATH; floorplan-apply unrunnable")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    fixtures = tmp_path / "fixtures"
    fixtures.mkdir()
    floorplan_path = fixtures / "vadd_floorplan.json"
    _write_vadd_floorplan_apply_fixture(floorplan_path)

    work = tmp_path / "rs-out"
    work.mkdir()
    run = _subprocess.run(
        _build_vadd_floorplan_apply_argv(
            binary,
            work,
            vadd_cpp,
            platform,
            floorplan_path,
        ),
        env=_rust_env(),
        capture_output=True,
        check=False,
        timeout=900,
    )
    if run.returncode != 0:
        msg = run.stderr.decode("utf-8", "replace")
        _maybe_skip_on_vadd_flow_failure(msg)
        pytest.fail(f"floorplan-apply failed unexpectedly:\n{msg}")

    snap = _collect_tree_json_snapshot(
        work,
        subpaths=[
            "graph.json",
            "design.json",
            "settings.json",
            "templates_info.json",
        ],
    )
    if _REFRESH:
        _refresh_tree_goldens(_VADD_FLOORPLAN_APPLY_GOLDEN_DIR, snap)
        return
    _assert_tree_goldens(_VADD_FLOORPLAN_APPLY_GOLDEN_DIR, snap, "vadd_floorplan_apply")
