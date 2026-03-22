"""Tests for Verilator Xilinx IP replacement helpers."""

from pathlib import Path
from tempfile import TemporaryDirectory

from tapa.cosim.verilator_ips import (
    detect_fp_operation_from_name,
    detect_xilinx_ips,
    generate_fp_ip_replacement,
    parse_ip_tcl,
)


def test_parse_ip_tcl_and_generate_replacement() -> None:
    with TemporaryDirectory() as tmp_dir:
        tcl_path = Path(tmp_dir) / "floating_point_0_ip.tcl"
        tcl_path.write_text(
            """create_ip -name floating_point
CONFIG.a_precision_type Double
CONFIG.operation_type Multiply
CONFIG.c_latency 7
""",
            encoding="utf-8",
        )

        config = parse_ip_tcl(tcl_path)
        assert config == {"dpi_func": "fp64_mul", "latency": 7}

        rendered = generate_fp_ip_replacement("floating_point_0_ip", "fp64_mul", 7)
        assert "module floating_point_0_ip" in rendered
        assert "fp64_mul" in rendered
        assert "reg [63:0] pipe [0:6];" in rendered


def test_detect_xilinx_ips_rewrites_protected_module() -> None:
    with TemporaryDirectory() as tmp_dir:
        rtl_dir = Path(tmp_dir)
        (rtl_dir / "top.v").write_text(
            "floating_point_0_ip ip0 (\n);\n", encoding="utf-8"
        )
        (rtl_dir / "floating_point_0_ip.v").write_text(
            "`pragma protect\n", encoding="utf-8"
        )
        (rtl_dir / "floating_point_0_ip.tcl").write_text(
            """create_ip -name floating_point
CONFIG.operation_type Add
""",
            encoding="utf-8",
        )

        replacements = detect_xilinx_ips(rtl_dir)
        assert replacements == ["floating_point_0_ip"]
        rewritten = (rtl_dir / "floating_point_0_ip.v").read_text(encoding="utf-8")
        assert "fp32_add" in rewritten


def test_detect_fp_operation_from_name_falls_back_to_heuristics() -> None:
    assert detect_fp_operation_from_name("foo_dmul_bar_ip") == "fp64_mul"
