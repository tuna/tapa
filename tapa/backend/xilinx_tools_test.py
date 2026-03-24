"""Characterization tests for xilinx_tools process wrapper contract.

Locks in:
- Context manager protocol (__enter__/__exit__) for Vivado and VivadoHls
- returncode propagation from the underlying ToolProcess
- communicate() returns (bytes, bytes)
"""

from __future__ import annotations

import os
import tempfile
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any
from unittest.mock import MagicMock, patch

import pytest

from tapa.backend.xilinx_tools import Vivado, VivadoHls
from tapa.remote.popen import LocalToolProcess, ToolProcess

if TYPE_CHECKING:
    from collections.abc import Generator

# Exit code used in tests that verify non-zero propagation.
_EXIT_CODE_42 = 42
_EXIT_CODE_7 = 7
_EXIT_CODE_99 = 99
_TUPLE_LEN_2 = 2


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_local_proc(cmd: list[str] | None = None) -> LocalToolProcess:
    """Create a LocalToolProcess wrapping a trivial command."""
    if cmd is None:
        cmd = ["true"]
    return LocalToolProcess(cmd)


def _make_mock_proc() -> MagicMock:
    """Return a MagicMock that looks like a ToolProcess."""
    mock_proc = MagicMock(spec=ToolProcess)
    mock_proc.returncode = None
    mock_proc.__enter__ = MagicMock(return_value=mock_proc)
    mock_proc.__exit__ = MagicMock(return_value=None)
    mock_proc.communicate = MagicMock(return_value=(b"stdout", b"stderr"))
    return mock_proc


@contextmanager
def _mock_create_tool_process() -> Generator[tuple[MagicMock, MagicMock]]:
    """Patch create_tool_process and get_remote_config; yield (mock_create, mock_proc).

    Yields a tuple of (mock_create, mock_proc).
    """
    mock_proc = _make_mock_proc()
    with (
        patch("tapa.backend.xilinx_tools.get_remote_config", return_value=None),
        patch(
            "tapa.backend.xilinx_tools.create_tool_process",
            return_value=mock_proc,
        ) as mock_create,
    ):
        yield mock_create, mock_proc


def _make_vivado_with_mock(mock_create: MagicMock) -> Any:  # noqa: ANN401
    """Instantiate Vivado with create_tool_process already mocked."""
    mock_proc = _make_mock_proc()
    mock_create.return_value = mock_proc
    vivado = Vivado("puts hello")
    return vivado, mock_proc


def _make_vivado_hls_with_mock(mock_create: MagicMock) -> Any:  # noqa: ANN401
    """Instantiate VivadoHls with create_tool_process already mocked."""
    mock_proc = _make_mock_proc()
    mock_create.return_value = mock_proc
    hls = VivadoHls("puts hello", hls="vivado_hls")
    return hls, mock_proc


# ---------------------------------------------------------------------------
# ToolProcess ABC contract
# ---------------------------------------------------------------------------


def test_tool_process_is_abstract() -> None:
    """ToolProcess cannot be instantiated directly."""
    with pytest.raises(TypeError):
        ToolProcess()  # type: ignore[abstract]


def test_local_tool_process_is_tool_process() -> None:
    """LocalToolProcess inherits from ToolProcess."""
    assert issubclass(LocalToolProcess, ToolProcess)


# ---------------------------------------------------------------------------
# LocalToolProcess context manager
# ---------------------------------------------------------------------------


def test_local_tool_process_context_manager_with_statement() -> None:
    """LocalToolProcess works correctly inside a with statement."""
    with _make_local_proc() as proc:
        assert isinstance(proc, LocalToolProcess)


def test_local_tool_process_context_manager_enter_returns_self() -> None:
    """LocalToolProcess context manager returns the same instance."""
    proc = _make_local_proc()
    with proc as entered:
        assert entered is proc


# ---------------------------------------------------------------------------
# LocalToolProcess returncode propagation
# ---------------------------------------------------------------------------


def test_local_tool_process_returncode_after_communicate() -> None:
    """Returncode reflects the subprocess returncode after communicate()."""
    proc = _make_local_proc(["sh", "-c", f"exit {_EXIT_CODE_42}"])
    stdout, stderr = proc.communicate()
    assert proc.returncode == _EXIT_CODE_42
    assert isinstance(stdout, bytes)
    assert isinstance(stderr, bytes)


def test_local_tool_process_returncode_zero_on_success() -> None:
    """Returncode is 0 for a command that exits successfully."""
    proc = _make_local_proc(["true"])
    proc.communicate()
    assert proc.returncode == 0


def test_local_tool_process_returncode_after_exit() -> None:
    """Returncode is set after the context manager exits."""
    proc = _make_local_proc(["sh", "-c", f"exit {_EXIT_CODE_7}"])
    with proc as p:
        p.communicate()
    assert proc.returncode == _EXIT_CODE_7


# ---------------------------------------------------------------------------
# LocalToolProcess communicate() return type
# ---------------------------------------------------------------------------


def test_local_tool_process_communicate_returns_bytes_tuple() -> None:
    """communicate() returns a 2-tuple of bytes."""
    proc = _make_local_proc(["echo", "hello"])
    result = proc.communicate()
    assert isinstance(result, tuple)
    assert len(result) == _TUPLE_LEN_2
    stdout, stderr = result
    assert isinstance(stdout, bytes)
    assert isinstance(stderr, bytes)


def test_local_tool_process_communicate_captures_stdout() -> None:
    """communicate() captures stdout bytes."""
    proc = _make_local_proc(["echo", "hello"])
    stdout, _ = proc.communicate()
    assert b"hello" in stdout


# ---------------------------------------------------------------------------
# Vivado wrapper — context manager and returncode delegation
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_context_manager_delegates_to_proc(mock_create: MagicMock) -> None:
    """Vivado context manager delegates enter/exit to its underlying _proc."""
    vivado, mock_proc = _make_vivado_with_mock(mock_create)
    with vivado:
        mock_proc.__enter__.assert_called_once()
    mock_proc.__exit__.assert_called_once()


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_returncode_propagates(mock_create: MagicMock) -> None:
    """Vivado.returncode reflects the underlying proc's returncode."""
    vivado, mock_proc = _make_vivado_with_mock(mock_create)
    mock_proc.returncode = 0
    assert vivado.returncode == 0

    mock_proc.returncode = 1
    assert vivado.returncode == 1


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_returncode_setter_propagates(mock_create: MagicMock) -> None:
    """Setting Vivado.returncode propagates to the underlying proc."""
    vivado, mock_proc = _make_vivado_with_mock(mock_create)
    vivado.returncode = _EXIT_CODE_99
    assert mock_proc.returncode == _EXIT_CODE_99


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_communicate_returns_bytes_tuple(mock_create: MagicMock) -> None:
    """Vivado.communicate() returns (bytes, bytes) from the underlying proc."""
    vivado, mock_proc = _make_vivado_with_mock(mock_create)
    mock_proc.communicate.return_value = (b"out", b"err")
    result = vivado.communicate()
    assert result == (b"out", b"err")


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_exit_cleans_up_tmpdir(mock_create: MagicMock) -> None:
    """Vivado context manager exit cleans up the temporary working directory."""
    vivado, _ = _make_vivado_with_mock(mock_create)
    cwd_name = vivado.cwd.name
    assert os.path.isdir(cwd_name)
    with vivado:
        pass
    assert not os.path.isdir(cwd_name)


# ---------------------------------------------------------------------------
# VivadoHls wrapper — context manager and returncode delegation
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_hls_context_manager_delegates_to_proc(mock_create: MagicMock) -> None:
    """VivadoHls context manager delegates enter/exit to its underlying _proc."""
    hls, mock_proc = _make_vivado_hls_with_mock(mock_create)
    with hls:
        mock_proc.__enter__.assert_called_once()
    mock_proc.__exit__.assert_called_once()


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_hls_returncode_propagates(mock_create: MagicMock) -> None:
    """VivadoHls.returncode reflects the underlying proc's returncode."""
    hls, mock_proc = _make_vivado_hls_with_mock(mock_create)
    mock_proc.returncode = 0
    assert hls.returncode == 0

    mock_proc.returncode = 1
    assert hls.returncode == 1


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_hls_communicate_returns_bytes_tuple(mock_create: MagicMock) -> None:
    """VivadoHls.communicate() returns (bytes, bytes) from the underlying proc."""
    hls, mock_proc = _make_vivado_hls_with_mock(mock_create)
    mock_proc.communicate.return_value = (b"o", b"e")
    result = hls.communicate()
    assert result == (b"o", b"e")


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_hls_exit_cleans_up_tmpdir(mock_create: MagicMock) -> None:
    """VivadoHls context manager exit cleans up the temporary working directory."""
    hls, _ = _make_vivado_hls_with_mock(mock_create)
    assert isinstance(hls.cwd, tempfile.TemporaryDirectory)
    cwd_name = hls.cwd.name
    assert os.path.isdir(cwd_name)
    with hls:
        pass
    assert not os.path.isdir(cwd_name)


# ---------------------------------------------------------------------------
# VivadoHls with explicit cwd — no tmpdir cleanup
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("_mock_remote_cfg")
@patch("tapa.backend.xilinx_tools.create_tool_process")
def test_vivado_hls_explicit_cwd_not_cleaned_up(mock_create: MagicMock) -> None:
    """VivadoHls does not clean up an explicitly provided cwd string."""
    mock_proc = _make_mock_proc()
    mock_create.return_value = mock_proc

    with tempfile.TemporaryDirectory() as explicit_cwd:
        hls = VivadoHls("puts hello", hls="vivado_hls", cwd=explicit_cwd)
        assert hls.cwd == explicit_cwd  # stored as plain str, not TemporaryDirectory
        with hls:
            pass
        # The directory was NOT cleaned up by VivadoHls
        assert os.path.isdir(explicit_cwd)


# ---------------------------------------------------------------------------
# Pytest fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def _mock_remote_cfg() -> Generator[None]:
    """Patch get_remote_config to return None (local mode) for the test."""
    with patch("tapa.backend.xilinx_tools.get_remote_config", return_value=None):
        yield
