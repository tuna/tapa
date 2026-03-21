"""Data types of module definitions."""

from tapa.graphir.types.modules.definitions.any import AnyModuleDefinition
from tapa.graphir.types.modules.definitions.aux import (
    AuxModuleDefinition,
    AuxSplitModuleDefinition,
)
from tapa.graphir.types.modules.definitions.base import BaseModuleDefinition
from tapa.graphir.types.modules.definitions.grouped import GroupedModuleDefinition
from tapa.graphir.types.modules.definitions.internal import (
    InternalGroupedModuleDefinition,
    InternalModuleDefinition,
    InternalVerilogModuleDefinition,
)
from tapa.graphir.types.modules.definitions.pass_through import (
    PassThroughModuleDefinition,
)
from tapa.graphir.types.modules.definitions.stub import StubModuleDefinition
from tapa.graphir.types.modules.definitions.verilog import VerilogModuleDefinition

__all__ = [
    "AnyModuleDefinition",
    "AuxModuleDefinition",
    "AuxSplitModuleDefinition",
    "BaseModuleDefinition",
    "GroupedModuleDefinition",
    "InternalGroupedModuleDefinition",
    "InternalModuleDefinition",
    "InternalVerilogModuleDefinition",
    "PassThroughModuleDefinition",
    "StubModuleDefinition",
    "VerilogModuleDefinition",
]
