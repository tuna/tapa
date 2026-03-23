"""Unit tests for tapa.common.floorplan.gen_slot_cpp."""

import pytest

from tapa.common.floorplan.gen_slot_cpp import replace_function


def test_replace_extern_c_function_replaces_body() -> None:
    source = """
extern "C" void my_func(int a, float b) {
    // original body
    int x = 1;
}
"""
    new_body = "#pragma HLS interface ap_none port = a register\n  { auto val = a; }"
    result = replace_function(source, "my_func", new_body)
    assert "pragma HLS" in result
    assert "original body" not in result


def test_replace_function_not_found_raises() -> None:
    source = """
extern "C" void other_func(int a) {
    int x = 1;
}
"""
    with pytest.raises(ValueError, match="not found"):
        replace_function(source, "nonexistent_func", "{ }")


def test_replace_function_preserves_surrounding_code() -> None:
    source = """
#include <stdint.h>

extern "C" void slot_func(uint64_t x) {
    // old body
}

// some trailing code
"""
    new_body = "// new body"
    result = replace_function(source, "slot_func", new_body)
    assert "#include <stdint.h>" in result
    assert "// some trailing code" in result
    assert "// new body" in result
    assert "// old body" not in result
