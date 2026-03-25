"""Characterization tests for tapa/program/hls.py."""

import io
import sys
import tarfile
import tempfile
from pathlib import Path
from types import TracebackType
from typing import Never, Self
from unittest.mock import MagicMock, patch

import pytest

from tapa.backend.xilinx import RunHls
from tapa.program.hls import ProgramHlsMixin, _gen_connections
from tapa.task import Task

# ---------------------------------------------------------------------------
# _gen_connections tests
# ---------------------------------------------------------------------------


def _make_upper_task_with_streams(
    task_name: str = "top",
    child_name: str = "sub",
    fifo_name: str = "fifo0",
) -> Task:
    """Build a minimal upper-level Task with one istream and one ostream."""
    return Task(
        name=task_name,
        code="",
        level="upper",
        tasks={
            child_name: [
                {
                    "args": {
                        "in_arg": {"cat": "istream", "arg": fifo_name},
                    },
                }
            ],
        },
        fifos={fifo_name: {}},
        ports=[],
    )


def test_gen_connections_returns_list() -> None:
    """_gen_connections always returns a list (possibly empty)."""
    task = Task(
        name="top",
        code="",
        level="upper",
        tasks={},
        fifos={},
        ports=[],
    )
    result = _gen_connections(task)
    assert isinstance(result, list)


def test_gen_connections_stream_pair_produces_connect_def() -> None:
    """A matched stream pair across two kernels produces a connect<stream> line."""
    task = Task(
        name="top",
        code="",
        level="upper",
        tasks={
            "producer": [
                {
                    "args": {
                        "out_arg": {"cat": "ostream", "arg": "fifo0"},
                    }
                }
            ],
            "consumer": [
                {
                    "args": {
                        "in_arg": {"cat": "istream", "arg": "fifo0"},
                    }
                }
            ],
        },
        fifos={"fifo0": {}},
        ports=[],
    )
    result = _gen_connections(task)
    assert isinstance(result, list)
    # There must be exactly one connect<stream> entry for the matched FIFO.
    stream_lines = [line for line in result if "connect<stream>" in line]
    assert len(stream_lines) == 1
    # The line must match the exact format including the k_ prefix.
    assert (
        stream_lines[0]
        == "connect<stream> fifo0 (k_producer0.out[0], k_consumer0.in[0]);"
    )


def test_gen_connections_unmatched_fifo_not_included() -> None:
    """A FIFO with only a source (no destination) must not appear in the output."""
    task = Task(
        name="top",
        code="",
        level="upper",
        tasks={
            "producer": [
                {
                    "args": {
                        "out_arg": {"cat": "ostream", "arg": "fifo_orphan"},
                    }
                }
            ],
        },
        fifos={"fifo_orphan": {}},
        ports=[],
    )
    result = _gen_connections(task)
    # No matched destination → no connect line for this FIFO.
    assert all("fifo_orphan" not in line for line in result)


def test_gen_connections_unknown_cat_raises() -> None:
    """_gen_connections raises ValueError for an unknown connection category."""
    task = Task(
        name="top",
        code="",
        level="upper",
        tasks={
            "child": [
                {
                    "args": {
                        "bad_arg": {"cat": "unknown_cat", "arg": "some_sig"},
                    }
                }
            ],
        },
        fifos={},
        ports=[],
    )
    with pytest.raises(ValueError, match="Unknown connection category"):
        _gen_connections(task)


def test_run_hls_returncode_property_settable() -> None:
    """The returncode setter delegates to _proc and is required by the retry loop."""
    mock_proc = MagicMock()
    mock_proc.returncode = 0

    with (
        patch("tapa.backend.xilinx_tools.create_tool_process", return_value=mock_proc),
        patch("tapa.backend.xilinx_hls.create_tool_process", return_value=mock_proc),
        tempfile.TemporaryDirectory() as tmpdir,
        tempfile.NamedTemporaryFile(suffix=".cpp") as cpp_file,
    ):
        tarfileobj = io.BytesIO()
        runner = RunHls(
            tarfileobj=tarfileobj,
            kernel_files=[(cpp_file.name, "")],
            work_dir=tmpdir,
            top_name="test_kernel",
            clock_period="10",
            part_num="xcu250",
        )
        # Verify returncode property reads from _proc.
        assert runner.returncode == mock_proc.returncode
        # Verify returncode setter propagates to _proc.
        runner.returncode = 1
        assert mock_proc.returncode == 1


def test_run_hls_exit_writes_tar_on_success(tmp_path: Path) -> None:
    """On returncode=0, __exit__ must write report/ and hdl/ entries to tarfileobj."""
    project_path = tmp_path / "kernel"
    solution_dir = project_path / "project" / "kernel"
    (solution_dir / "syn" / "report").mkdir(parents=True)
    (solution_dir / "syn" / "verilog").mkdir(parents=True)
    # The log path resolves to project_path/vivado_hls.log because project_path
    # is an absolute path and os.path.join discards preceding segments.
    log_file = project_path / "vivado_hls.log"
    log_file.write_bytes(b"")

    mock_proc = MagicMock()
    mock_proc.returncode = 0
    mock_proc.__enter__ = MagicMock(return_value=mock_proc)
    mock_proc.__exit__ = MagicMock(return_value=None)

    tarfileobj = io.BytesIO()
    with (
        patch("tapa.backend.xilinx_tools.create_tool_process", return_value=mock_proc),
        patch("tapa.backend.xilinx_hls.create_tool_process", return_value=mock_proc),
        patch("tapa.backend.xilinx_tools.get_remote_config", return_value=None),
    ):
        runner = RunHls(
            tarfileobj=tarfileobj,
            kernel_files=[],
            work_dir=str(tmp_path),
            top_name="kernel",
            clock_period="5",
            part_num="xcu250",
        )
        runner.__exit__(None, None, None)

    # Verify tar was written with the expected arcnames
    tarfileobj.seek(0)
    with tarfile.open(fileobj=tarfileobj, mode="r") as tar:
        names = tar.getnames()
    assert any(n.startswith("report") for n in names), f"No report/ in tar: {names}"
    assert any(n.startswith("hdl") for n in names), f"No hdl/ in tar: {names}"


# ---------------------------------------------------------------------------
# Retry (recursion) behaviour in the HLS worker
# ---------------------------------------------------------------------------


class _ConcreteHlsMixin(ProgramHlsMixin):
    """Minimal concrete subclass so we can instantiate ProgramHlsMixin."""

    top = "mykernel"
    cflags = ""
    work_dir = "/tmp/fake_work"

    def __init__(self, tasks: dict) -> None:
        self._tasks = tasks

    # ProgramDirectoryInterface stubs — intentionally static-like but must be
    # instance methods to satisfy the abstract interface.
    def get_cpp_path(self, name: str) -> str:  # noqa: PLR6301
        return f"/tmp/fake/{name}.cpp"

    def get_tar_path(self, name: str) -> str:  # noqa: PLR6301
        return f"/tmp/fake/{name}.tar"

    def get_header_path(self, name: str) -> str:  # noqa: PLR6301
        return f"/tmp/fake/{name}.h"

    def get_common_path(self) -> str:  # noqa: PLR6301
        return "/tmp/fake/common.h"

    # ProgramInterface stub — not needed for this test
    @property
    def top_task(self) -> Never:
        raise NotImplementedError


# Number of times RunHls must be instantiated in the retry scenario.
_EXPECTED_RETRY_CALL_COUNT = 2


def test_worker_retries_on_pre_synthesis_failure() -> None:
    """worker() calls RunHls a second time on a flaky Pre-synthesis failure."""
    task = Task(name="mykernel", code="", level="lower")
    mixin = _ConcreteHlsMixin(tasks={"mykernel": task})

    run_hls_call_count = {"n": 0}

    class _FakeRunHls:
        """Mock RunHls that succeeds on the second call."""

        def __init__(self, *_args: object, **_kwargs: object) -> None:
            call_idx = run_hls_call_count["n"]
            run_hls_call_count["n"] += 1
            self._rc = 1 if call_idx == 0 else 0
            self._stdout = b"Pre-synthesis failed." if call_idx == 0 else b""

        def __enter__(self) -> Self:
            return self

        def __exit__(
            self,
            _exc_type: type[BaseException] | None,
            _exc_val: BaseException | None,
            _tb: TracebackType | None,
        ) -> None:
            return None

        def communicate(self) -> tuple[bytes, bytes]:
            return (self._stdout, b"")

        @property
        def returncode(self) -> int:
            return self._rc

    with (
        patch("tapa.program.hls.RunHls", _FakeRunHls),
        patch("tapa.program.hls.get_remote_config", return_value=None),
        patch("tapa.program.hls.find_resource", side_effect=FileNotFoundError),
        patch(
            "builtins.open",
            MagicMock(
                return_value=MagicMock(
                    __enter__=MagicMock(return_value=MagicMock()),
                    __exit__=MagicMock(return_value=None),
                )
            ),
        ),
    ):
        mixin.run_hls(
            clock_period="10",
            part_num="xcu250",
            skip_based_on_mtime=False,
            other_configs="",
            jobs=1,
            keep_hls_work_dir=False,
        )

    # The worker must have been called twice: once failing, once succeeding.
    assert run_hls_call_count["n"] == _EXPECTED_RETRY_CALL_COUNT, (
        f"Expected RunHls to be instantiated {_EXPECTED_RETRY_CALL_COUNT} times "
        f"(retry), but was called {run_hls_call_count['n']} time(s)"
    )


def test_worker_does_not_retry_when_error_line_present() -> None:
    """worker() must NOT retry when stdout has both Pre-synthesis and ERROR: markers."""
    task = Task(name="mykernel", code="", level="lower")
    mixin = _ConcreteHlsMixin(tasks={"mykernel": task})

    run_hls_call_count = {"n": 0}

    class _FakeRunHlsWithError:
        """Mock RunHls that always reports a hard ERROR."""

        def __init__(self, *_args: object, **_kwargs: object) -> None:
            run_hls_call_count["n"] += 1

        def __enter__(self) -> Self:
            return self

        def __exit__(
            self,
            _exc_type: type[BaseException] | None,
            _exc_val: BaseException | None,
            _tb: TracebackType | None,
        ) -> None:
            return None

        def communicate(self) -> tuple[bytes, bytes]:  # noqa: PLR6301
            # Both markers present → no retry, raises RuntimeError
            return (b"Pre-synthesis failed.\nERROR: something bad", b"")

        @property
        def returncode(self) -> int:
            return 1

    with (
        patch("tapa.program.hls.RunHls", _FakeRunHlsWithError),
        patch("tapa.program.hls.get_remote_config", return_value=None),
        patch("tapa.program.hls.find_resource", side_effect=FileNotFoundError),
        patch(
            "builtins.open",
            MagicMock(
                return_value=MagicMock(
                    __enter__=MagicMock(return_value=MagicMock()),
                    __exit__=MagicMock(return_value=None),
                )
            ),
        ),
        patch.object(sys, "exit"),
        patch.object(sys.stdout, "write"),
        patch.object(sys.stderr, "write"),
    ):
        mixin.run_hls(
            clock_period="10",
            part_num="xcu250",
            skip_based_on_mtime=False,
            other_configs="",
            jobs=1,
            keep_hls_work_dir=False,
        )

    # No retry: RunHls was only instantiated once.
    assert run_hls_call_count["n"] == 1, (
        f"Expected RunHls instantiated once (no retry), "
        f"but was called {run_hls_call_count['n']} time(s)"
    )
