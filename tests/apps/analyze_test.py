"""Test that tapa analyze produces meaningful output for each app."""

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass

import pytest

VALID_PORT_CATS = {"istream", "istreams", "ostream", "ostreams", "mmap", "scalar"}
VALID_LEVELS = {"upper", "lower"}


@dataclass
class AppConfig:
    """Configuration for a test app."""

    name: str
    source: str
    top_name: str
    expected_tasks: list[str]
    requires_vendor: bool = False


APPS = [
    AppConfig(
        name="vadd",
        source="tests/apps/vadd/vadd.cpp",
        top_name="VecAdd",
        expected_tasks=["VecAdd", "Mmap2Stream", "Add", "Stream2Mmap"],
    ),
    AppConfig(
        name="bandwidth",
        source="tests/apps/bandwidth/bandwidth.cpp",
        top_name="Bandwidth",
        expected_tasks=["Bandwidth"],
        requires_vendor=True,  # requires ap_int.h from Vitis HLS
    ),
    AppConfig(
        name="cannon",
        source="tests/apps/cannon/cannon.cpp",
        top_name="Cannon",
        expected_tasks=["Cannon", "Gather", "ProcElem", "Scatter"],
    ),
    AppConfig(
        name="gemv",
        source="tests/apps/gemv/gemv.cpp",
        top_name="Gemv",
        expected_tasks=["Gemv"],
        requires_vendor=True,  # requires ap_int.h from Vitis HLS
    ),
    AppConfig(
        name="graph",
        source="tests/apps/graph/graph.cpp",
        top_name="Graph",
        expected_tasks=["Graph", "Control", "ProcElem", "UpdateHandler"],
    ),
    AppConfig(
        name="jacobi",
        source="tests/apps/jacobi/jacobi.cpp",
        top_name="Jacobi",
        expected_tasks=["Jacobi", "Mmap2Stream", "Stream2Mmap"],
    ),
    AppConfig(
        name="network",
        source="tests/apps/network/network.cpp",
        top_name="Network",
        expected_tasks=["Network", "Consume", "Produce", "Switch2x2"],
    ),
]

HAS_VENDOR_HEADERS = bool(
    os.environ.get("XILINX_HLS") or os.environ.get("XILINX_VITIS")
)


def _find_tapa() -> str:
    """Find the tapa binary in Bazel runfiles or fall back to PATH."""
    for env_var in ("RUNFILES_DIR", "TEST_SRCDIR"):
        base = os.environ.get(env_var, "")
        if base:
            tapa = os.path.join(base, "_main", "tapa", "tapa")
            if os.path.isfile(tapa):
                return tapa
    return "tapa"


def _find_source(rel_path: str) -> str:
    """Resolve a workspace-relative source path via Bazel runfiles."""
    runfiles = os.environ.get("TEST_SRCDIR", "")
    if runfiles:
        full = os.path.join(runfiles, "_main", rel_path)
        if os.path.isfile(full):
            return full
    return rel_path


def _find_tapa_lib_include() -> str | None:
    """Return the tapa-lib include directory from Bazel runfiles, if present."""
    runfiles = os.environ.get("TEST_SRCDIR", "")
    if runfiles:
        inc = os.path.join(runfiles, "_main", "tapa-lib")
        if os.path.isdir(inc):
            return inc
    return None


def _validate_task(task: dict, task_name: str, app_name: str) -> None:
    """Validate the structure of a single task in the graph."""
    ctx = f"{app_name}/{task_name}"
    assert "level" in task, f"{ctx}: missing 'level'"
    assert task["level"] in VALID_LEVELS, f"{ctx}: invalid level '{task['level']}'"
    assert "target" in task, f"{ctx}: missing 'target'"
    assert "code" in task, f"{ctx}: missing 'code'"
    assert task["code"], f"{ctx}: empty code"
    assert "ports" in task, f"{ctx}: missing 'ports'"
    assert task["ports"], f"{ctx}: no ports"
    for port in task["ports"]:
        assert "name" in port, f"{ctx}: port missing 'name'"
        assert "cat" in port, f"{ctx}/{port['name']}: missing 'cat'"
        assert port["cat"] in VALID_PORT_CATS, (
            f"{ctx}/{port['name']}: invalid cat '{port['cat']}'"
        )
    if task["level"] == "upper":
        assert "tasks" in task, f"{ctx}: upper-level task missing 'tasks'"
        assert task["tasks"], f"{ctx}: upper-level task has no subtasks"
        assert "fifos" in task, f"{ctx}: upper-level task missing 'fifos'"


@pytest.mark.parametrize("app", APPS, ids=[app.name for app in APPS])
def test_analyze(app: AppConfig) -> None:
    """Test that tapa analyze produces a graph with expected tasks."""
    if app.requires_vendor and not HAS_VENDOR_HEADERS:
        pytest.skip("requires Vitis HLS vendor headers (XILINX_HLS)")

    tapa_bin = _find_tapa()
    src_path = _find_source(app.source)

    with tempfile.TemporaryDirectory(prefix=f"tapa-analyze-{app.name}-") as work_dir:
        cmd = [
            tapa_bin,
            "--work-dir",
            work_dir,
            "analyze",
            "--input",
            src_path,
            "--top",
            app.top_name,
        ]
        cmd.extend(["--cflags", f"-I{os.path.dirname(src_path)}"])
        tapa_lib_inc = _find_tapa_lib_include()
        if tapa_lib_inc:
            cmd.extend(["--cflags", f"-I{tapa_lib_inc}"])

        result = subprocess.run(
            cmd, check=False, capture_output=True, text=True, timeout=120
        )
        assert result.returncode == 0, (
            f"tapa analyze failed for {app.name}:\n"
            f"stdout: {result.stdout}\nstderr: {result.stderr}"
        )

        graph_path = os.path.join(work_dir, "graph.json")
        assert os.path.isfile(graph_path), f"graph.json missing for {app.name}"
        assert os.path.isfile(os.path.join(work_dir, "settings.json")), (
            f"settings.json missing for {app.name}"
        )
        assert os.path.isdir(os.path.join(work_dir, "flatten")), (
            f"flatten/ dir missing for {app.name}"
        )

        with open(graph_path, encoding="utf-8") as f:
            graph = json.load(f)

        assert "tasks" in graph, f"graph.json missing 'tasks' for {app.name}"
        task_names = list(graph["tasks"].keys())
        assert task_names, f"No tasks in graph for {app.name}"

        for expected in app.expected_tasks:
            assert expected in task_names, (
                f"Expected task '{expected}' not in {app.name}. Found: {task_names}"
            )

        for task_name, task in graph["tasks"].items():
            _validate_task(task, task_name, app.name)

        assert graph["tasks"][app.top_name]["level"] == "upper", (
            f"{app.name}: top task '{app.top_name}' should be upper-level"
        )


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-v"]))
