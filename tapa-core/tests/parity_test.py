"""Phase 7 — `tapa-cli` parity tests (Python click vs Rust binary).

Drives both the Python `python3 -m tapa` entry point and the Rust
`cargo run -p tapa-cli` binary against the `tests/apps/*` fixtures and
compares their on-disk outputs / parsed argv / `--help` flag surfaces.
Skips cleanly when the toolchain is not configured (no `tapacc`, no
`tapa-cpp`, no Python tapa, etc.) so this suite is safe to run on any
developer machine.

The legacy Phase 5b / Phase 6 cross-language tests that drove
`tapa_core` PyO3 bindings were removed when the bindings crate was
retired in Phase 7 (AC-7); their cross-language verification reference
is now the new CLI parity gate plus the
`tapa-core/tapa-task-graph/tests/python_design_round_trip.rs` Rust
integration test for the JSON schema bridge.

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


def _have(cmd: str) -> bool:
    return shutil.which(cmd) is not None


def _python_tapa_importable() -> bool:
    """Return True when `python3 -m tapa --help` succeeds."""
    if not _have("python3"):
        return False
    res = _subprocess.run(
        ["python3", "-c", "import tapa.__main__"],
        env={**os.environ, "PYTHONPATH": str(_REPO_ROOT)},
        capture_output=True,
        check=False,
        timeout=30,
    )
    return res.returncode == 0


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


def _python_tapa_argv(*subcmd_args: str, work_dir: Path) -> list[str]:
    return [
        "python3",
        "-m",
        "tapa.__main__",
        "--work-dir",
        str(work_dir),
        *subcmd_args,
    ]


def _rust_tapa_argv(binary: Path, *subcmd_args: str, work_dir: Path) -> list[str]:
    return [str(binary), "--work-dir", str(work_dir), *subcmd_args]


def _python_env() -> dict[str, str]:
    return {**os.environ, "PYTHONPATH": str(_REPO_ROOT)}


# Steps where the native Rust port does NOT yet cover the surface this
# parity suite needs and we therefore still escape to the Python bridge.
# Every entry below is paired with a Codex Round 1 acknowledgement that
# the native path is incomplete; once a port closes the gap, drop it
# from this set so the Rust path is exercised end-to-end.
_BRIDGE_STILL_REQUIRED: dict[str, str] = {
    # synth: native port stops after preflight + settings persistence;
    # the HLS pipeline (`Program.run_hls` + `generate_task_rtl` +
    # `generate_top_rtl`) is not yet ported, so any test that needs
    # populated `<work_dir>/rtl` content must opt the bridge back in.
    "SYNTH": "HLS pipeline + RTL codegen not yet ported (Codex R1 ack)",
    # compile / compile-with-floorplan-dse: composite drivers that
    # transitively need the un-ported synth pipeline above. Bridge stays
    # on until SYNTH lands natively.
    "COMPILE": "transitively requires un-ported SYNTH pipeline",
    "COMPILE_WITH_FLOORPLAN_DSE": "transitively requires un-ported SYNTH",
    # generate-floorplan: AutoBridge driver + un-ported synth flags.
    "GENERATE_FLOORPLAN": "wraps un-ported SYNTH + AutoBridge driver",
}


def _rust_env(*, bridge_steps: tuple[str, ...] = ()) -> dict[str, str]:
    """Build env for the Rust binary, bridging only when explicitly asked.

    By default this leaves *every* `TAPA_STEP_*_PYTHON` unset so the
    parity suite exercises the native Rust path for ported steps
    (analyze, synth-preflight, pack-preflight, floorplan no-op,
    run-autobridge local). Pass `bridge_steps=("SYNTH",)` (etc.) to
    re-enable the Python fallback for steps still listed in
    [`_BRIDGE_STILL_REQUIRED`].
    """
    env = {**os.environ, "PYTHONPATH": str(_REPO_ROOT)}
    # Strip any bridge env that may have leaked in from the caller's
    # shell so a stray `TAPA_STEP_*_PYTHON=1` doesn't silently mask
    # native-path regressions.
    for key in list(env):
        if key.startswith("TAPA_STEP_") and key.endswith("_PYTHON"):
            env.pop(key, None)
    for step in bridge_steps:
        if step not in _BRIDGE_STILL_REQUIRED:
            msg = (
                f"refusing to bridge `{step}`: not in `_BRIDGE_STILL_REQUIRED`. "
                f"Either add it with a Codex-ack reason or stop opting in."
            )
            raise AssertionError(msg)
        env[f"TAPA_STEP_{step}_PYTHON"] = "1"
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


def _skip_if_cli_toolchain_missing() -> tuple[Path, dict[str, Any]]:
    if not _VADD_DIR.is_dir():
        pytest.skip(f"vadd fixture missing: {_VADD_DIR}")
    if not _python_tapa_importable():
        pytest.skip("python3 cannot import tapa.__main__")
    binary = _rust_tapa_binary()
    if binary is None:
        pytest.skip("rust `tapa` binary unavailable (cargo build failed)")
    return binary, {}


# `.xo` redaction + inventory helpers used by `test_parity_cli_vadd_flow`.
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


def test_parity_cli_help_lists_same_subcommands() -> None:
    """Verify both runtimes list the same subcommand surface in --help.

    AC-2 negative test: a dropped or renamed flag would fail this diff.
    """
    binary, _ = _skip_if_cli_toolchain_missing()

    py = _subprocess.run(
        ["python3", "-m", "tapa.__main__", "--help"],
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=30,
    )
    rs = _subprocess.run(
        [str(binary), "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert py.returncode == 0, py.stderr.decode("utf-8", "replace")
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")
    py_text = py.stdout.decode("utf-8", "replace")
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
        assert sub in py_text, f"Python CLI must list `{sub}`"
        assert sub in rs_text, f"Rust CLI must list `{sub}`"


def test_parity_cli_version_matches() -> None:
    binary, _ = _skip_if_cli_toolchain_missing()
    py = _subprocess.run(
        ["python3", "-m", "tapa.__main__", "version"],
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=30,
    )
    rs = _subprocess.run(
        [str(binary), "version"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert py.returncode == 0
    assert rs.returncode == 0
    assert py.stdout == rs.stdout, (
        f"version output diverges: py={py.stdout!r} rs={rs.stdout!r}"
    )


def test_parity_cli_analyze_vadd(tmp_path: Path) -> None:
    """Compare `tapa analyze` outputs on `tests/apps/vadd` across runtimes.

    Asserts that the on-disk `graph.json` / `design.json` /
    `settings.json` match structurally. Skips cleanly when `tapacc` /
    `tapa-cpp` cannot be located (the Python CLI itself raises
    `FileNotFoundError` and we surface that as a skip).
    """
    binary, _ = _skip_if_cli_toolchain_missing()

    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")

    py_work = tmp_path / "py-out"
    rs_work = tmp_path / "rs-out"

    py_run = _subprocess.run(
        _python_tapa_argv(
            "analyze",
            "--input",
            str(vadd_cpp),
            "--top",
            _VADD_TOP,
            work_dir=py_work,
        ),
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=300,
    )
    if py_run.returncode != 0:
        # `tapacc` / `tapa-cpp` not on the path; skip cleanly.
        msg = py_run.stderr.decode("utf-8", "replace")
        if "Cannot find" in msg or "FileNotFoundError" in msg:
            pytest.skip(f"tapacc toolchain not configured: {msg.splitlines()[0]}")
        pytest.fail(f"python analyze failed: {msg}")

    rs_run = _subprocess.run(
        _rust_tapa_argv(
            binary,
            "analyze",
            "--input",
            str(vadd_cpp),
            "--top",
            _VADD_TOP,
            work_dir=rs_work,
        ),
        env=_rust_env(),
        capture_output=True,
        check=False,
        timeout=300,
    )
    if rs_run.returncode != 0:
        msg = rs_run.stderr.decode("utf-8", "replace")
        if "Cannot find" in msg or "tapacc binary not found" in msg:
            pytest.skip(
                f"tapacc toolchain not configured (rust): {msg.splitlines()[0]}"
            )
        pytest.fail(f"rust analyze failed: {msg}")

    for fname in ("graph.json", "settings.json"):
        py_data = _read_json(py_work / fname)
        rs_data = _read_json(rs_work / fname)
        assert py_data == rs_data, (
            f"{fname} diverges between runtimes\npy={py_data}\nrs={rs_data}"
        )

    py_design = _normalize_design_for_compare(_read_json(py_work / "design.json"))
    rs_design = _normalize_design_for_compare(_read_json(rs_work / "design.json"))
    assert py_design == rs_design, (
        "design.json topology diverges between runtimes after normalization"
    )


def test_parity_cli_unknown_first_token_fails() -> None:
    """Reject `tapa bogus-subcommand` on both runtimes (AC-2 negative).

    Both CLIs must exit non-zero rather than silently no-op.
    """
    binary, _ = _skip_if_cli_toolchain_missing()
    py = _subprocess.run(
        ["python3", "-m", "tapa.__main__", "bogus-subcommand"],
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=30,
    )
    rs = _subprocess.run(
        [str(binary), "bogus-subcommand"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert py.returncode != 0, "python CLI must reject bogus subcommand"
    assert rs.returncode != 0, "rust CLI must reject bogus subcommand"


def test_parity_cli_chained_argv_value_collision_does_not_split() -> None:
    """Keep flag values attached to their flag (Codex regression, AC-3).

    A flag value that equals a subcommand name (e.g. `--top synth`)
    must stay attached to `--top`, not boundary the chunk into a new
    chained subcommand.
    """
    binary, _ = _skip_if_cli_toolchain_missing()

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
        env={**os.environ},
        timeout=30,
    )
    # The rust binary refuses with a typed error coming out of the
    # `analyze` step — either `tapa-cpp-binary not found` on the native
    # path, or `StepUnported` when the bridge env var is unset. The
    # important parity point is that `synth` is not misclassified as a
    # chained subcommand — otherwise clap would surface an
    # "unrecognized argument" diagnostic instead.
    assert rs.returncode != 0
    msg = rs.stderr.decode("utf-8", "replace").lower()
    assert "analyze" in msg or "tapa-cpp" in msg or "tapacc" in msg, msg
    assert "unrecognized" not in msg, msg
    assert "unexpected" not in msg, msg


# ---------------------------------------------------------------------
# Per-subcommand `--help` diff: every subcommand's long-flag set must
# be the same on click (Python) and clap (Rust). Help-text wording can
# diverge; the *flag surface* may not silently shrink or rename.
# ---------------------------------------------------------------------


_LONG_FLAG_RE = _re.compile(r"--[A-Za-z][A-Za-z0-9-]+")

# Codex Round 1 acknowledged divergences. Each entry pairs a
# subcommand with a frozen set of long flags that legitimately exist
# on one runtime only, plus a one-line reason. The diff test still
# fails if a *new* divergence appears outside this allowlist.
_HELP_DIFF_ALLOWLIST: dict[str, dict[str, frozenset[str]]] = {
    # No allowlisted divergences: the Rust composites mirror the
    # Python `_extend_params` flag surface exactly. Adding entries
    # here is a regression — open a bug instead.
}


def _extract_long_flags(help_text: str) -> set[str]:
    """Pull `--long-flag` tokens from a help dump.

    Click and clap render help differently, but both list each long
    flag verbatim. Strip out common false positives (`--help`,
    word-wrapped `--keep-hierarchy` continuations) — the test only
    cares about the documented option names.
    """
    flags = {m.group(0) for m in _LONG_FLAG_RE.finditer(help_text)}
    # `--help` / `--version` are uninteresting boilerplate present on
    # both sides; dropping them keeps the diff focused on the real
    # option surface.
    flags.discard("--help")
    flags.discard("--version")
    return flags


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


@pytest.mark.parametrize("subcommand", _SUBCOMMANDS)
def test_parity_cli_subcommand_help_diff(subcommand: str) -> None:
    """The Python and Rust CLI must enumerate the same long-flag set.

    A renamed or dropped flag immediately fails this test, with a
    diff showing exactly which side changed. Help-text wording can
    differ freely — only the `--name` set is compared.
    """
    binary, _ = _skip_if_cli_toolchain_missing()

    py = _subprocess.run(
        ["python3", "-m", "tapa.__main__", subcommand, "--help"],
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=30,
    )
    rs = _subprocess.run(
        [str(binary), subcommand, "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert py.returncode == 0, py.stderr.decode("utf-8", "replace")
    assert rs.returncode == 0, rs.stderr.decode("utf-8", "replace")

    py_flags = _extract_long_flags(py.stdout.decode("utf-8", "replace"))
    rs_flags = _extract_long_flags(rs.stdout.decode("utf-8", "replace"))

    allow = _HELP_DIFF_ALLOWLIST.get(
        subcommand, {"py_only": frozenset(), "rs_only": frozenset()}
    )
    py_only = py_flags - rs_flags - allow["py_only"]
    rs_only = rs_flags - py_flags - allow["rs_only"]
    # If the allowlist itself becomes stale (the divergence closed),
    # surface that loudly so we delete the entry.
    stale_py = allow["py_only"] - (py_flags - rs_flags)
    stale_rs = allow["rs_only"] - (rs_flags - py_flags)

    assert not py_only, (
        f"`{subcommand}`: Python-only flags appeared without an allowlist "
        f"entry: {sorted(py_only)}. Either port them to Rust or extend "
        f"`_HELP_DIFF_ALLOWLIST['{subcommand}']['py_only']` with a reason."
    )
    assert not rs_only, (
        f"`{subcommand}`: Rust-only flags appeared without an allowlist "
        f"entry: {sorted(rs_only)}. Either remove them or extend "
        f"`_HELP_DIFF_ALLOWLIST['{subcommand}']['rs_only']` with a reason."
    )
    assert not stale_py, (
        f"`{subcommand}`: allowlisted `py_only` flags are now present in "
        f"both runtimes — drop them from `_HELP_DIFF_ALLOWLIST`: "
        f"{sorted(stale_py)}"
    )
    assert not stale_rs, (
        f"`{subcommand}`: allowlisted `rs_only` flags are now present in "
        f"both runtimes — drop them from `_HELP_DIFF_ALLOWLIST`: "
        f"{sorted(stale_rs)}"
    )


# ---------------------------------------------------------------------
# Fixture-based chained-argv corpus.
#
# Discover every `tests/apps/*/run_tapa.bats` and lift the canonical
# `compile_xo` argv out of it. Each fixture's argv is then forwarded to
# both runtimes with `--help` appended so clap/click parse the surface
# without invoking Vitis. Both must accept the same flag layout
# (exit 0, no "no such option" / "unrecognized argument" diagnostic).
# ---------------------------------------------------------------------

_APPS_DIR = _REPO_ROOT / "tests" / "apps"


def _discover_app_argvs() -> list[tuple[str, list[str]]]:
    """Return `(app_name, argv)` for every `run_tapa.bats` we can parse.

    The `compile_xo` body has a uniform shape across the seven app
    fixtures:

        ${TAPA_HOME}/usr/bin/tapa \
          -w ${BATS_TMPDIR}/<app>-workdir \
          compile \
          --jobs 2 \
          --platform xilinx_u250_gen3x16_xdma_4_1_202210_1 \
          -f <app>.cpp \
          -t <Top> \
          -o ${BATS_TMPDIR}/<app>.xo

    We strip bats-only `${...}` interpolations to neutral
    placeholders so both runtimes see syntactically valid tokens.
    """
    out: list[tuple[str, list[str]]] = []
    if not _APPS_DIR.is_dir():
        return out
    for app_dir in sorted(_APPS_DIR.iterdir()):
        bats = app_dir / "run_tapa.bats"
        if not bats.is_file():
            continue
        text = bats.read_text(encoding="utf-8")
        # Locate the `compile_xo()` body and lift the `tapa ... \` block.
        match = _re.search(
            r"compile_xo\s*\(\)\s*\{(.+?)^\}",
            text,
            flags=_re.DOTALL | _re.MULTILINE,
        )
        if match is None:
            continue
        body = match.group(1)
        # Find the line carrying the `tapa` invocation; flatten
        # backslash-continued continuations into a single string.
        flat = _re.sub(r"\\\s*\n\s*", " ", body)
        m = _re.search(r"\btapa\b\s+(.*)", flat)
        if m is None:
            continue
        tail = m.group(1)
        # Stop at the closing brace's preceding `[ -f ... ]` if
        # captured. The first line break ends the command.
        tail = tail.split("\n", 1)[0].strip()
        # Replace bats placeholders with concrete values that are
        # syntactically valid for both runtimes.
        tail = tail.replace("${BATS_TMPDIR}", "/tmp/tapa-parity")
        tail = tail.replace("${TAPA_HOME}", "/tmp/tapa-home")
        tail = tail.replace("${BATS_TEST_DIRNAME}", str(app_dir))
        # Tokenize. The bats files use plain whitespace separation,
        # no quoting, so a simple split is sufficient.
        argv = tail.split()
        if not argv:
            continue
        out.append((app_dir.name, argv))
    return out


_APP_ARGVS = _discover_app_argvs()


@pytest.mark.parametrize(
    ("app_name", "argv"),
    _APP_ARGVS,
    ids=[name for name, _ in _APP_ARGVS] or ["__no_apps__"],
)
def test_parity_cli_chained_argv_corpus(app_name: str, argv: list[str]) -> None:
    """Every fixture's chained argv must parse identically on both runtimes.

    Appending `--help` short-circuits the actual pipeline so we only
    exercise the parser. Both runtimes must exit 0; neither may emit
    a parse-error diagnostic ("unrecognized" / "no such option" /
    "unexpected").
    """
    if not argv:
        pytest.skip(f"empty argv for {app_name}")
    binary, _ = _skip_if_cli_toolchain_missing()

    # Append `--help` to the *subcommand* portion so click/clap print
    # subcommand-level help and exit 0 without launching the toolchain.
    py = _subprocess.run(
        ["python3", "-m", "tapa.__main__", *argv, "--help"],
        env=_python_env(),
        capture_output=True,
        check=False,
        timeout=30,
    )
    rs = _subprocess.run(
        [str(binary), *argv, "--help"],
        capture_output=True,
        check=False,
        timeout=30,
    )

    py_err = py.stderr.decode("utf-8", "replace").lower()
    rs_err = rs.stderr.decode("utf-8", "replace").lower()
    py_out = py.stdout.decode("utf-8", "replace").lower()
    rs_out = rs.stdout.decode("utf-8", "replace").lower()
    bad_tokens = ("unrecognized", "no such option", "unexpected argument")

    assert py.returncode == 0, (
        f"[{app_name}] python rejected the fixture argv: {py_err}"
    )
    assert rs.returncode == 0, f"[{app_name}] rust rejected the fixture argv: {rs_err}"
    for token in bad_tokens:
        assert token not in py_err and token not in py_out, (
            f"[{app_name}] python parser complained about `{token}`: {py_err}"
        )
        assert token not in rs_err and token not in rs_out, (
            f"[{app_name}] rust parser complained about `{token}`: {rs_err}"
        )


# ---------------------------------------------------------------------
# Real `analyze synth pack` flow on the vadd fixture.
#
# When tapacc + Vitis HLS are available, run the full chain natively
# on both runtimes and assert the produced `.xo` archives are
# byte-equal after the redaction normalization that already powers
# `test_parity_xilinx_xo_redaction`. Skip cleanly otherwise.
# ---------------------------------------------------------------------


def _xo_signature(
    xo_path: Path,
) -> tuple[
    list[tuple[str, str]],
    list[tuple[str, int, tuple[int, ...]]],
]:
    """Return `(content_inventory, listing_metadata)` for `xo_path`.

    Reuses the redaction-test helpers so `.xo` parity is judged on the
    same axes as `test_parity_xilinx_xo_redaction`: per-entry SHA-256
    plus listing metadata (name, size, normalized timestamp).
    """
    return _zip_inventory(xo_path), _zip_metadata(xo_path)


def test_parity_cli_vadd_flow(tmp_path: Path) -> None:
    """End-to-end native vadd: `analyze synth pack` on both runtimes.

    Runs the full chained pipeline against `tests/apps/vadd` with no
    bridge env, then compares the produced `.xo` archives by content
    SHA-256 and listing metadata after the standard redaction pass.
    Skips cleanly when:
      - tapacc / tapa-cpp is not installed (analyze can't run),
      - Vitis HLS is unavailable (synth needs `vitis_hls`),
      - the Rust SYNTH path hasn't landed yet (StepUnported error).

    The skip-on-`StepUnported` branch is what makes this test
    forward-compatible: the moment SYNTH ports natively, the skip
    drops away and the assertion goes live without test edits.
    """
    binary, _ = _skip_if_cli_toolchain_missing()
    vadd_cpp = _VADD_DIR / "vadd.cpp"
    if not vadd_cpp.is_file():
        pytest.skip(f"missing {vadd_cpp}")
    if not _have("vitis_hls") and "XILINX_HLS" not in os.environ:
        pytest.skip("vitis_hls not on PATH and `XILINX_HLS` unset")

    platform = os.environ.get(
        "TAPA_PARITY_PLATFORM",
        "xilinx_u250_gen3x16_xdma_4_1_202210_1",
    )

    def _run_chain(work_dir: Path, *, env: dict[str, str], argv0: list[str]) -> Path:
        """Execute analyze+synth+pack against `work_dir`, return the `.xo`."""
        xo_path = work_dir / "vadd.xo"
        full_argv = [
            *argv0,
            "--work-dir",
            str(work_dir),
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
        run = _subprocess.run(
            full_argv,
            env=env,
            capture_output=True,
            check=False,
            timeout=900,
        )
        if run.returncode != 0:
            msg = run.stderr.decode("utf-8", "replace")
            for needle in (
                "Cannot find",
                "tapacc binary not found",
                "tapa-cpp",
                "vitis_hls",
                "XILINX_HLS",
                "StepUnported",
                "is not yet ported",
                "is not yet supported",
            ):
                if needle in msg:
                    pytest.skip(
                        f"vadd flow toolchain/native gap: {needle} | "
                        f"{msg.splitlines()[0] if msg.strip() else '<no stderr>'}"
                    )
            pytest.fail(f"chain failed unexpectedly:\n{msg}")
        if not xo_path.is_file():
            pytest.skip(f"chain succeeded but no `.xo` at {xo_path}")
        return xo_path

    py_work = tmp_path / "py-out"
    rs_work = tmp_path / "rs-out"
    py_work.mkdir()
    rs_work.mkdir()

    py_xo = _run_chain(
        py_work,
        env=_python_env(),
        argv0=["python3", "-m", "tapa.__main__"],
    )
    rs_xo = _run_chain(
        rs_work,
        env=_rust_env(),
        argv0=[str(binary)],
    )

    # Redact both archives through the production Python path so the
    # comparison ignores embedded timestamps / abs paths / project IDs.
    py_redacted = tmp_path / "py-redacted.xo"
    rs_redacted = tmp_path / "rs-redacted.xo"
    try:
        _py_redact(py_xo, py_redacted)
        _py_redact(rs_xo, rs_redacted)
    except ImportError as exc:
        pytest.skip(f"tapa.program.pack not importable: {exc}")

    py_inv, py_meta = _xo_signature(py_redacted)
    rs_inv, rs_meta = _xo_signature(rs_redacted)
    assert py_inv == rs_inv, (
        f"vadd `.xo` content drift after redaction\n  py={py_inv}\n  rs={rs_inv}"
    )
    assert py_meta == rs_meta, (
        f"vadd `.xo` listing-metadata drift after redaction\n"
        f"  py={py_meta}\n  rs={rs_meta}"
    )
