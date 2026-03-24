"""Characterization tests for config_preprocess routing logic."""

import zipfile
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import pytest

from tapa.cosim.config_preprocess import CosimConfig, _parse_and_update_config


def _make_dummy_zip(path: str) -> None:
    """Create a minimal zip file at `path` (valid zip, empty contents)."""
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("dummy.txt", "placeholder")


def _make_config(xo_path: str) -> CosimConfig:
    return CosimConfig(xo_path=xo_path)


def test_xo_extension_sets_vitis_mode() -> None:
    """When xo_path ends in .xo, _parse_and_update_config sets mode='vitis'."""
    with TemporaryDirectory() as tmp_xo_dir, TemporaryDirectory() as tb_output_dir:
        xo_path = str(Path(tmp_xo_dir) / "kernel.xo")
        _make_dummy_zip(xo_path)
        config = _make_config(xo_path)

        with (
            patch("tapa.cosim.config_preprocess._parse_xo_update_config") as mock_xo,
            patch("tapa.cosim.config_preprocess._remap_keys", return_value={}),
        ):
            # _parse_xo_update_config must populate config.args for _remap_keys
            mock_xo.side_effect = lambda cfg, _tmp: setattr(cfg, "args", [])
            _parse_and_update_config(config, tb_output_dir)

        assert config.mode == "vitis"
        mock_xo.assert_called_once()


def test_zip_extension_sets_hls_mode() -> None:
    """When xo_path ends in .zip, _parse_and_update_config sets mode='hls'."""
    with TemporaryDirectory() as tmp_xo_dir, TemporaryDirectory() as tb_output_dir:
        xo_path = str(Path(tmp_xo_dir) / "kernel.zip")
        _make_dummy_zip(xo_path)
        config = _make_config(xo_path)

        with (
            patch("tapa.cosim.config_preprocess._parse_zip_update_config") as mock_zip,
            patch("tapa.cosim.config_preprocess._remap_keys", return_value={}),
        ):
            mock_zip.side_effect = lambda cfg, _tmp: setattr(cfg, "args", [])
            _parse_and_update_config(config, tb_output_dir)

        assert config.mode == "hls"
        mock_zip.assert_called_once()


def test_xo_extension_does_not_call_zip_handler() -> None:
    """When xo_path ends in .xo, the zip handler must NOT be called."""
    with TemporaryDirectory() as tmp_xo_dir, TemporaryDirectory() as tb_output_dir:
        xo_path = str(Path(tmp_xo_dir) / "kernel.xo")
        _make_dummy_zip(xo_path)
        config = _make_config(xo_path)

        with (
            patch("tapa.cosim.config_preprocess._parse_xo_update_config") as mock_xo,
            patch("tapa.cosim.config_preprocess._parse_zip_update_config") as mock_zip,
            patch("tapa.cosim.config_preprocess._remap_keys", return_value={}),
        ):
            mock_xo.side_effect = lambda cfg, _tmp: setattr(cfg, "args", [])
            _parse_and_update_config(config, tb_output_dir)

        mock_zip.assert_not_called()


def test_zip_extension_does_not_call_xo_handler() -> None:
    """When xo_path ends in .zip, the xo handler must NOT be called."""
    with TemporaryDirectory() as tmp_xo_dir, TemporaryDirectory() as tb_output_dir:
        xo_path = str(Path(tmp_xo_dir) / "kernel.zip")
        _make_dummy_zip(xo_path)
        config = _make_config(xo_path)

        with (
            patch("tapa.cosim.config_preprocess._parse_xo_update_config") as mock_xo,
            patch("tapa.cosim.config_preprocess._parse_zip_update_config") as mock_zip,
            patch("tapa.cosim.config_preprocess._remap_keys", return_value={}),
        ):
            mock_zip.side_effect = lambda cfg, _tmp: setattr(cfg, "args", [])
            _parse_and_update_config(config, tb_output_dir)

        mock_xo.assert_not_called()


def test_unknown_extension_raises_value_error() -> None:
    """When xo_path has an unsupported extension, a ValueError must be raised."""
    with TemporaryDirectory() as tmp_xo_dir, TemporaryDirectory() as tb_output_dir:
        xo_path = str(Path(tmp_xo_dir) / "kernel.tar")
        _make_dummy_zip(xo_path)
        config = _make_config(xo_path)

        with pytest.raises(ValueError, match="Unsupported xo file format"):
            _parse_and_update_config(config, tb_output_dir)
