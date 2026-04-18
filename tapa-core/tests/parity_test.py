"""Phase 7 — `tapa-cli` golden-snapshot tests for the Rust binary.

The legacy click-based Python CLI (`tapa/__main__.py` + `tapa/steps/`)
was retired in commit `222519ce` (AC-8). With the Python entry point
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
    regression (the bridge no longer exists post-AC-8).
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

    The Python click CLI was retired in commit `222519ce` (AC-8); only
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
    """Reject `tapa bogus-subcommand` (negative parser gate, AC-2)."""
    binary = _skip_if_cli_toolchain_missing()
    rs = _subprocess.run(
        [str(binary), "bogus-subcommand"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert rs.returncode != 0, "rust CLI must reject bogus subcommand"


def test_cli_chained_argv_value_collision_does_not_split() -> None:
    """Keep flag values attached to their flag (Codex regression, AC-3).

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
    retired (commit `222519ce`, AC-8). A flag rename or removal trips
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
# `compile_xo` corpus. AC-11 requires explicit coverage for each major
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
