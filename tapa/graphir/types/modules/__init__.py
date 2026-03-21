"""Data types of modules."""

from tapa.graphir.types.modules.definitions import (
    AnyModuleDefinition,
    AuxModuleDefinition,
    AuxSplitModuleDefinition,
    BaseModuleDefinition,
    GroupedModuleDefinition,
    InternalGroupedModuleDefinition,
    InternalModuleDefinition,
    InternalVerilogModuleDefinition,
    PassThroughModuleDefinition,
    StubModuleDefinition,
    VerilogModuleDefinition,
)
from tapa.graphir.types.modules.instantiation import ModuleInstantiation
from tapa.graphir.types.modules.supports import (
    ModuleConnection,
    ModuleNet,
    ModuleParameter,
    ModulePort,
)

__all__ = [
    "AnyModuleDefinition",
    "AuxModuleDefinition",
    "AuxSplitModuleDefinition",
    "BaseModuleDefinition",
    "GroupedModuleDefinition",
    "InternalGroupedModuleDefinition",
    "InternalModuleDefinition",
    "InternalVerilogModuleDefinition",
    "ModuleConnection",
    "ModuleInstantiation",
    "ModuleNet",
    "ModuleParameter",
    "ModulePort",
    "PassThroughModuleDefinition",
    "StubModuleDefinition",
    "VerilogModuleDefinition",
]
