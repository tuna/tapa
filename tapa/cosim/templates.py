"""Backward-compatible aliases for the render module.

All ``get_*`` names delegate directly to their ``render_*`` counterparts in
:mod:`tapa.cosim.render`.  New code should import from that module instead.
"""

from tapa.cosim.render import (
    render_axi_ram_inst as get_axi_ram_inst,
)
from tapa.cosim.render import (
    render_axi_ram_module as get_axi_ram_module,
)
from tapa.cosim.render import (
    render_axis as get_axis,
)
from tapa.cosim.render import (
    render_fifo as get_fifo,
)
from tapa.cosim.render import (
    render_hls_dut as get_hls_dut,
)
from tapa.cosim.render import (
    render_hls_test_signals as get_hls_test_signals,
)
from tapa.cosim.render import (
    render_m_axi_connections as get_m_axi_connections,
)
from tapa.cosim.render import (
    render_s_axi_control as get_s_axi_control,
)
from tapa.cosim.render import (
    render_srl_fifo_template as get_srl_fifo_template,
)
from tapa.cosim.render import (
    render_stream_typedef as get_stream_typedef,
)
from tapa.cosim.render import (
    render_testbench_begin as get_begin,
)
from tapa.cosim.render import (
    render_testbench_end as get_end,
)
from tapa.cosim.render import (
    render_vitis_dut as get_vitis_dut,
)
from tapa.cosim.render import (
    render_vitis_test_signals as get_vitis_test_signals,
)

# FIXME: test if using addr_width = 64 will cause problem in simulation

__all__ = [
    "get_axi_ram_inst",
    "get_axi_ram_module",
    "get_axis",
    "get_begin",
    "get_end",
    "get_fifo",
    "get_hls_dut",
    "get_hls_test_signals",
    "get_m_axi_connections",
    "get_s_axi_control",
    "get_srl_fifo_template",
    "get_stream_typedef",
    "get_vitis_dut",
    "get_vitis_test_signals",
]
