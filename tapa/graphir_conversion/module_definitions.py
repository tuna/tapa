"""Static GraphIR module definitions for FIFO, reset inverter, and ctrl_s_axi."""

from pathlib import Path

from tapa.graphir.types import (
    Expression,
    HierarchicalName,
    ModuleConnection,
    ModuleInstantiation,
    ModuleParameter,
    ModulePort,
    Range,
    Token,
    VerilogModuleDefinition,
)
from tapa.graphir_conversion.templates import FIFO_TEMPLATE, RESET_INVERTER_TEMPLATE
from tapa.graphir_conversion.utils import get_verilog_definition_from_tapa_module
from tapa.task import Task
from tapa.verilog.xilinx.module import Module

_CTRL_S_AXI_PORT_DIR_RANGE = {
    "ACLK": (ModulePort.Type.INPUT, None),
    "ARESET": (ModulePort.Type.INPUT, None),
    "ACLK_EN": (ModulePort.Type.INPUT, None),
    "AWADDR": (
        ModulePort.Type.INPUT,
        Range(
            left=Expression(
                (
                    Token.new_id("C_S_AXI_ADDR_WIDTH"),
                    Token.new_lit("-"),
                    Token.new_lit("1"),
                )
            ),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "AWVALID": (ModulePort.Type.INPUT, None),
    "AWREADY": (ModulePort.Type.OUTPUT, None),
    "WDATA": (
        ModulePort.Type.INPUT,
        Range(
            left=Expression(
                (
                    Token.new_id("C_S_AXI_DATA_WIDTH"),
                    Token.new_lit("-"),
                    Token.new_lit("1"),
                )
            ),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "WSTRB": (
        ModulePort.Type.INPUT,
        Range(
            left=Expression(
                (
                    Token.new_id("C_S_AXI_DATA_WIDTH"),
                    Token.new_lit("/"),
                    Token.new_lit("8"),
                    Token.new_lit("-"),
                    Token.new_lit("1"),
                )
            ),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "WVALID": (ModulePort.Type.INPUT, None),
    "WREADY": (ModulePort.Type.OUTPUT, None),
    "BRESP": (
        ModulePort.Type.OUTPUT,
        Range(
            left=Expression((Token.new_lit("1"),)),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "BVALID": (ModulePort.Type.OUTPUT, None),
    "BREADY": (ModulePort.Type.INPUT, None),
    "ARADDR": (
        ModulePort.Type.INPUT,
        Range(
            left=Expression(
                (
                    Token.new_id("C_S_AXI_ADDR_WIDTH"),
                    Token.new_lit("-"),
                    Token.new_lit("1"),
                )
            ),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "ARVALID": (ModulePort.Type.INPUT, None),
    "ARREADY": (ModulePort.Type.OUTPUT, None),
    "RDATA": (
        ModulePort.Type.OUTPUT,
        Range(
            left=Expression(
                (
                    Token.new_id("C_S_AXI_DATA_WIDTH"),
                    Token.new_lit("-"),
                    Token.new_lit("1"),
                )
            ),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "RRESP": (
        ModulePort.Type.OUTPUT,
        Range(
            left=Expression((Token.new_lit("1"),)),
            right=Expression((Token.new_lit("0"),)),
        ),
    ),
    "RVALID": (ModulePort.Type.OUTPUT, None),
    "RREADY": (ModulePort.Type.INPUT, None),
    "interrupt": (ModulePort.Type.OUTPUT, None),
    "ap_start": (ModulePort.Type.OUTPUT, None),
    "ap_done": (ModulePort.Type.INPUT, None),
    "ap_ready": (ModulePort.Type.INPUT, None),
    "ap_idle": (ModulePort.Type.INPUT, None),
}

_CTRL_S_AXI_PARAMETERS = [
    ModuleParameter(
        name="C_S_AXI_ADDR_WIDTH",
        hierarchical_name=HierarchicalName.get_name("C_S_AXI_ADDR_WIDTH"),
        expr=Expression((Token.new_lit("6"),)),
        range=None,
    ),
    ModuleParameter(
        name="C_S_AXI_DATA_WIDTH",
        hierarchical_name=HierarchicalName.get_name("C_S_AXI_DATA_WIDTH"),
        expr=Expression((Token.new_lit("32"),)),
        range=None,
    ),
]


def get_ctrl_s_axi_def(top: Task, content: str) -> VerilogModuleDefinition:
    """Get control_s_axi module definition."""
    bit64_range = Range(
        left=Expression((Token.new_lit("63"),)),
        right=Expression((Token.new_lit("0"),)),
    )
    ports = [
        ModulePort(
            name=port_name,
            hierarchical_name=HierarchicalName.get_name(port_name),
            type=port_type,
            range=port_range,
        )
        for port_name, (port_type, port_range) in _CTRL_S_AXI_PORT_DIR_RANGE.items()
    ]
    for port_name, port in top.ports.items():
        ctrl_s_axi_port_name = (
            port_name if not port.cat.is_mmap else f"{port_name}_offset"
        )
        ports.append(
            ModulePort(
                name=ctrl_s_axi_port_name,
                hierarchical_name=HierarchicalName.get_name(ctrl_s_axi_port_name),
                type=ModulePort.Type.OUTPUT,
                range=bit64_range,
            )
        )
    name = f"{top.name}_control_s_axi"
    return VerilogModuleDefinition(
        name=name,
        hierarchical_name=HierarchicalName.get_name(name),
        parameters=tuple(_CTRL_S_AXI_PARAMETERS),
        ports=tuple(ports),
        verilog=content,
        submodules_module_names=(),
    )


def get_fifo_def() -> VerilogModuleDefinition:
    """Get fifo module definition."""
    data_range = Range(
        left=Expression(
            (
                Token.new_id("DATA_WIDTH"),
                Token.new_lit("-"),
                Token.new_lit("1"),
            )
        ),
        right=Expression((Token.new_lit("0"),)),
    )

    def _param(name: str, val: str) -> ModuleParameter:
        return ModuleParameter(
            name=name,
            hierarchical_name=HierarchicalName.get_name(name),
            expr=Expression((Token.new_lit(val),)),
            range=None,
        )

    def _port(
        name: str, hname: str, ptype: ModulePort.Type, prange: Range | None = None
    ) -> ModulePort:
        return ModulePort(
            name=name,
            hierarchical_name=HierarchicalName.get_name(hname),
            type=ptype,
            range=prange,
        )

    inp = ModulePort.Type.INPUT
    out = ModulePort.Type.OUTPUT
    return VerilogModuleDefinition(
        name="fifo",
        hierarchical_name=HierarchicalName.get_name("fifo"),
        parameters=(
            _param("DATA_WIDTH", "32"),
            _param("ADDR_WIDTH", "5"),
            _param("DEPTH", "32"),
        ),
        ports=(
            _port("clk", "clk", inp),
            _port("reset", "rst", inp),
            _port("if_full_n", "if_full_n", out),
            _port("if_write_ce", "if_write_ce", inp),
            _port("if_write", "if_write", inp),
            _port("if_din", "if_din", inp, data_range),
            _port("if_empty_n", "if_empty_n", out),
            _port("if_read_ce", "if_read_ce", inp),
            _port("if_read", "if_read", inp),
            _port("if_dout", "if_dout", out, data_range),
        ),
        verilog=FIFO_TEMPLATE,
        submodules_module_names=(),
    )


def get_fsm_def(fsm_file: Path) -> VerilogModuleDefinition:
    """Get FSM module definition."""
    content = Path(fsm_file).read_text(encoding="utf-8")
    module = Module((fsm_file,), is_trimming_enabled=True)
    return get_verilog_definition_from_tapa_module(module, content)


def get_reset_inverter_def() -> VerilogModuleDefinition:
    """Get reset inverter module definition."""
    inp = ModulePort.Type.INPUT
    out = ModulePort.Type.OUTPUT
    return VerilogModuleDefinition(
        name="reset_inverter",
        hierarchical_name=HierarchicalName.get_name("reset_inverter"),
        parameters=(),
        ports=(
            ModulePort(
                name="clk",
                hierarchical_name=HierarchicalName.get_name("clk"),
                type=inp,
                range=None,
            ),
            ModulePort(
                name="rst",
                hierarchical_name=HierarchicalName.get_name("rst"),
                type=out,
                range=None,
            ),
            ModulePort(
                name="rst_n",
                hierarchical_name=HierarchicalName.get_name("rst_n"),
                type=inp,
                range=None,
            ),
        ),
        verilog=RESET_INVERTER_TEMPLATE,
        submodules_module_names=(),
    )


def get_reset_inverter_inst(floorplan_region: str) -> ModuleInstantiation:
    """Get reset inverter module instantiation."""
    return ModuleInstantiation(
        name="reset_inverter_0",
        hierarchical_name=HierarchicalName.get_name("reset_inverter_0"),
        module="reset_inverter",
        parameters=(),
        connections=tuple(
            ModuleConnection(
                name=name,
                hierarchical_name=HierarchicalName.get_name(name),
                expr=Expression((Token.new_id(signal),)),
            )
            for name, signal in (
                ("clk", "ap_clk"),
                ("rst", "rst"),
                ("rst_n", "ap_rst_n"),
            )
        ),
        floorplan_region=floorplan_region,
        area=None,
    )
