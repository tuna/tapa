"""Data structure to represent the area of a module instance."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from pydantic import BaseModel, ConfigDict


class InstanceArea(BaseModel):
    """Resource usage of a module instance."""

    model_config = ConfigDict(frozen=True)

    ff: int
    lut: int
    dsp: int
    bram_18k: int
    uram: int

    def to_dict(self) -> dict[str, int]:
        """Return a dict representation of the area."""
        return {
            "FF": self.ff,
            "LUT": self.lut,
            "DSP": self.dsp,
            "BRAM_18K": self.bram_18k,
            "URAM": self.uram,
        }
