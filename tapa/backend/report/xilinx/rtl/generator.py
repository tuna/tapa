import os
import re
import shutil
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from types import TracebackType
from typing import BinaryIO, TextIO

from tapa.backend import xilinx as backend

__all__ = (
    "ReportDirUtil",
    "ReportXoUtil",
    "RtlHlsInfo",
)

REPORT_UTIL_COMMANDS = r"""
set hdl_dir [lindex $argv 0]
set rpt_file [lindex $argv 1]
set_param general.maxThreads 1
set_part {part_num}
read_verilog [ glob $hdl_dir/*.v ]
set ips [ glob -nocomplain $hdl_dir/*/*.xci ]
if {{ $ips ne "" }} {{
  import_ip $ips
  upgrade_ip [get_ips *]
  generate_target synthesis [ get_files *.xci ]
}}
foreach tcl_file [glob -nocomplain $hdl_dir/*.tcl] {{
  source $tcl_file
}}
{set_parallel}
synth_design {synth_args}
opt_design
report_utilization -file $rpt_file {report_util_args}
"""


class ReportDirUtil(backend.Vivado):
    """Run synthesis and generate resource utilization report."""

    def __init__(  # noqa: PLR0913
        self,
        hdl_dir: str,
        rpt_path: str,
        top_name: str,
        *,
        part_num: str | None = None,
        synth_kwargs: dict[str, str] | None = None,
        report_util_kwargs: dict[str, str] | None = None,
    ) -> None:
        if part_num is None:
            for hdl_file_format in ("{}.v", "{0}_{0}.v"):
                hdl_path = os.path.join(hdl_dir, hdl_file_format.format(top_name))
                try:
                    with open(hdl_path, encoding="utf-8") as hdl_file:
                        part_num = RtlHlsInfo(hdl_file)["HLS_INPUT_PART"]
                    break
                except FileNotFoundError:
                    pass
        if part_num is None:
            msg = "failed to infer part number"
            raise ValueError(msg)

        if synth_kwargs is None:
            synth_kwargs = {}
        synth_kwargs["top"] = top_name
        synth_kwargs["part"] = part_num

        if report_util_kwargs is None:
            report_util_kwargs = {}
        report_util_kwargs.setdefault("hierarchical", "")

        synth_args = " ".join(f"-{k} {v}" for k, v in synth_kwargs.items())
        report_util_args = " ".join(f"-{k} {v}" for k, v in report_util_kwargs.items())
        kwargs = {
            "part_num": part_num,
            "synth_args": synth_args,
            "report_util_args": report_util_args,
            "set_parallel": "",
        }
        abs_hdl_dir = os.path.abspath(hdl_dir)
        abs_rpt_path = os.path.abspath(rpt_path)
        rpt_dir = os.path.dirname(abs_rpt_path)
        os.makedirs(rpt_dir, exist_ok=True)
        self._extra_upload = (abs_hdl_dir, rpt_dir)
        self._extra_download = (rpt_dir,)
        super().__init__(
            REPORT_UTIL_COMMANDS.format(**kwargs), abs_hdl_dir, abs_rpt_path
        )


class ReportXoUtil(ReportDirUtil):
    """Run synthesis and generate resource utilization report from an XO file."""

    def __init__(  # noqa: PLR0913
        self,
        xo_file: BinaryIO,
        rpt_file: TextIO,
        *,
        top_name: str = "Dataflow",
        part_num: str | None = None,
        synth_kwargs: dict[str, str] | None = None,
        report_util_kwargs: dict[str, str] | None = None,
    ) -> None:
        self.tmpdir = tempfile.TemporaryDirectory(prefix="report-xo-util-")
        self.rpt_file = rpt_file
        self.rpt_file_name = os.path.join(self.tmpdir.name, "post_synth_util.rpt")
        with zipfile.ZipFile(xo_file) as xo_zip:
            with xo_zip.open("xo.xml") as xo_xml:
                kernel = ET.parse(xo_xml).find("./Kernels/Kernel")
                if kernel is None:
                    msg = "cannot parse XO file"
                    raise ValueError(msg)
                ip_dir = kernel.attrib["IP"]
                kernel_name = kernel.attrib["Name"]
            hdl_dir_prefix = ip_dir + "/src"
            if all(not x.startswith(hdl_dir_prefix) for x in xo_zip.namelist()):
                hdl_dir_prefix = ip_dir + "/hdl/verilog"
            hdl_dir = os.path.join(self.tmpdir.name, hdl_dir_prefix)
            xo_zip.extractall(
                path=self.tmpdir.name,
                members=[
                    name
                    for name in xo_zip.namelist()
                    if name.startswith(hdl_dir_prefix)
                ],
            )

        if not top_name:
            top_name = kernel_name

        super().__init__(
            hdl_dir,
            self.rpt_file_name,
            top_name,
            part_num=part_num,
            synth_kwargs=synth_kwargs,
            report_util_kwargs=report_util_kwargs,
        )

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        super().__exit__(exc_type, exc_value, traceback)
        try:
            with open(self.rpt_file_name, encoding="utf-8") as src_rpt_file:
                shutil.copyfileobj(src_rpt_file, self.rpt_file)
        except FileNotFoundError:
            msg = "failed to generate report file"
            raise ValueError(msg)
        self.tmpdir.cleanup()


RTL_HLS_INFO_REGEX = r'\(\* CORE_GENERATION_INFO\s*=\s*".*,\{(.*)\}" \*\)'


class RtlHlsInfo:
    """Parsed HLS generation info from an RTL file header."""

    def __init__(self, rtl_file: TextIO) -> None:
        match = re.search(RTL_HLS_INFO_REGEX, rtl_file.read())
        if match is None:
            msg = "cannot parse RTL file"
            raise ValueError(msg)
        self._data = dict(item.split("=") for item in match.group(1).split(","))

    def __getitem__(self, key: str) -> str:
        return self._data[key]
