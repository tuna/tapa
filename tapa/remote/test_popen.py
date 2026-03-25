"""Unit tests for RemoteToolProcess download_cwd behaviour."""

from typing import Any
from unittest.mock import MagicMock, patch

from tapa.remote.config import RemoteConfig
from tapa.remote.popen import RemoteToolProcess


def _fake_config() -> RemoteConfig:
    return RemoteConfig(host="fake-host", work_dir="/tmp/fake-remote")


def _make_proc(cwd: str, **kwargs: Any) -> RemoteToolProcess:  # noqa: ANN401
    return RemoteToolProcess(
        ["echo", "hello"],
        cwd=cwd,
        config=_fake_config(),
        **kwargs,
    )


@patch("tapa.remote.popen._download_paths")
@patch("tapa.remote.popen._upload_paths")
@patch("tapa.remote.popen.run_ssh_with_stdout")
def test_default_does_not_download_cwd(
    mock_ssh: MagicMock,
    _mock_upload: MagicMock,  # noqa: PT019
    mock_download: MagicMock,
) -> None:
    """download_cwd=False (default) must not include cwd in download list."""
    mock_ssh.return_value = (0, b"", b"")
    proc = _make_proc("/some/cwd")
    proc.communicate()
    mock_download.assert_called_once()
    downloaded = mock_download.call_args[0][1]  # positional arg: paths list
    assert "/some/cwd" not in downloaded


@patch("tapa.remote.popen._download_paths")
@patch("tapa.remote.popen._upload_paths")
@patch("tapa.remote.popen.run_ssh_with_stdout")
def test_download_cwd_true_includes_cwd(
    mock_ssh: MagicMock,
    _mock_upload: MagicMock,  # noqa: PT019
    mock_download: MagicMock,
) -> None:
    """download_cwd=True must include cwd in download list."""
    mock_ssh.return_value = (0, b"", b"")
    proc = _make_proc("/some/cwd", download_cwd=True)
    proc.communicate()
    mock_download.assert_called_once()
    downloaded = mock_download.call_args[0][1]
    assert "/some/cwd" in downloaded


@patch("tapa.remote.popen._download_paths")
@patch("tapa.remote.popen._upload_paths")
@patch("tapa.remote.popen.run_ssh_with_stdout")
def test_extra_download_paths_still_downloaded_without_cwd(
    mock_ssh: MagicMock,
    _mock_upload: MagicMock,  # noqa: PT019
    mock_download: MagicMock,
) -> None:
    """Extra download paths are always respected regardless of download_cwd."""
    mock_ssh.return_value = (0, b"", b"")
    proc = _make_proc(
        "/some/cwd",
        extra_download_paths=("/explicit/output",),
    )
    proc.communicate()
    mock_download.assert_called_once()
    downloaded = mock_download.call_args[0][1]
    assert "/some/cwd" not in downloaded
    assert "/explicit/output" in downloaded
