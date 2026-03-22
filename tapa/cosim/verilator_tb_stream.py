"""Stream queue code generation for Verilator TBs."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import Arg


def generate_stream_support(args: Sequence[Arg]) -> list[str]:
    stream_args = [arg for arg in args if arg.is_stream]
    if not stream_args:
        return []
    lines: list[str] = [
        "",
        "#include <queue>",
        "",
        "struct StreamQueue {",
        "    std::queue<std::vector<uint8_t>> data;",
        "    size_t width_bytes;",
        "    bool eot_sent = false;",
        "};",
        "",
    ]
    for arg in stream_args:
        width_bytes = (arg.port.data_width + 7) // 8
        lines.append(
            f"static StreamQueue stream_{arg.qualified_name}{{{{}}, {width_bytes}}};"
        )
    lines.extend(
        [
            "",
            ("static void load_stream(StreamQueue& sq, const char* path) {"),
            "    std::ifstream f(path, std::ios::binary);",
            "    if (!f) return;",
            "    while (f.peek() != EOF) {",
            "        std::vector<uint8_t> buf(sq.width_bytes);",
            ("        f.read(reinterpret_cast<char*>(buf.data()), sq.width_bytes);"),
            "        size_t n = f.gcount();",
            "        buf.resize(n);",
            "        if (n > 0) sq.data.push(std::move(buf));",
            "    }",
            "}",
            "",
            ("static void dump_stream(StreamQueue& sq, const char* path) {"),
            "    std::ofstream f(path, std::ios::binary);",
            "    while (!sq.data.empty()) {",
            "        auto& buf = sq.data.front();",
            ("        f.write(reinterpret_cast<const char*>(buf.data()), buf.size());"),
            "        sq.data.pop();",
            "    }",
            "}",
            "",
        ]
    )
    return lines
