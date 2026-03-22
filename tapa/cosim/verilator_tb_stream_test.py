"""Tests for Verilator stream-support generation."""

from tapa.cosim.common import Arg, Port
from tapa.cosim.verilator_tb_stream import generate_stream_support


def test_generate_stream_support_includes_stream_queue_helpers() -> None:
    args = [Arg("stream", 4, 0, Port("stream", "read_only", 32))]
    text = "\n".join(generate_stream_support(args))
    assert "static StreamQueue stream_stream_s" in text
    assert "static void load_stream(StreamQueue& sq, const char* path) {" in text
    assert "static void dump_stream(StreamQueue& sq, const char* path) {" in text
