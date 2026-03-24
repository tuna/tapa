"""Tests for cosim __main__ utilities."""

import pytest

from tapa.cosim.__main__ import _safe_eval_int

_DATA_WIDTH = 512
_DATA_WIDTH_MINUS_ONE = 511


def test_safe_eval_int_literal() -> None:
    assert _safe_eval_int(str(_DATA_WIDTH_MINUS_ONE), {}) == _DATA_WIDTH_MINUS_ONE


def test_safe_eval_int_subtraction() -> None:
    assert _safe_eval_int(f"{_DATA_WIDTH}-1", {}) == _DATA_WIDTH_MINUS_ONE


def test_safe_eval_int_with_param_substitution() -> None:
    assert (
        _safe_eval_int("C_M_AXI_DATA_WIDTH-1", {"C_M_AXI_DATA_WIDTH": str(_DATA_WIDTH)})
        == _DATA_WIDTH_MINUS_ONE
    )


def test_safe_eval_int_with_expression() -> None:
    """eval() replacement must handle Verilog parameter expressions safely."""
    # Simulate Verilog: parameter C_M_AXI_DATA_WIDTH = 512;
    # Interface: [C_M_AXI_DATA_WIDTH-1:0] width expression
    result = _safe_eval_int(
        "C_M_AXI_DATA_WIDTH-1", {"C_M_AXI_DATA_WIDTH": str(_DATA_WIDTH)}
    )
    assert result == _DATA_WIDTH_MINUS_ONE  # 512-1


_WHITESPACE_TEST_VALUE = 63


def test_safe_eval_int_with_leading_whitespace() -> None:
    """Verilog regex extraction may leave leading/trailing whitespace."""
    assert _safe_eval_int(f" {_WHITESPACE_TEST_VALUE} ", {}) == _WHITESPACE_TEST_VALUE


def test_safe_eval_int_rejects_function_call() -> None:
    with pytest.raises(ValueError, match="Unsafe expression"):
        _safe_eval_int("__import__('os')", {})


def test_safe_eval_int_rejects_attribute_access() -> None:
    with pytest.raises(ValueError, match="Unsafe expression"):
        _safe_eval_int("x.y", {"x": "1"})
