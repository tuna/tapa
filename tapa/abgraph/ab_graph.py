"""Graph based on Pydantic for the partitioning algorithm."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import networkx as nx
from pydantic import BaseModel, ConfigDict

from tapa.abgraph.device.common import RESOURCES, Coor
from tapa.abgraph.device.virtual_device import Area


class ABVertex(BaseModel):
    """Represents a vertex in the AutoBridge graph."""

    name: str
    sub_cells: tuple[str, ...]
    area: Area
    target_slot: str | None
    reserved_slot: str | None

    current_slot: Coor | None = None

    def __hash__(self) -> int:
        """Return a hash for the vertex."""
        return hash(self.name)

    def __eq__(self, other: object) -> bool:
        """Return whether two vertices are equal."""
        if not isinstance(other, ABVertex):
            return NotImplemented
        return self.name == other.name

    def __lt__(self, other: object) -> bool:
        """Return whether one vertex is less than another."""
        if not isinstance(other, ABVertex):
            return NotImplemented
        return self.name < other.name


class ABEdge(BaseModel):
    """Represents an edge in the AutoBridge graph."""

    model_config = ConfigDict(frozen=True)

    source_vertex: ABVertex
    target_vertex: ABVertex

    index: int
    width: int


class ABGraph(BaseModel):
    """Represents a graph in the AutoBridge partitioning algorithm."""

    vs: list[ABVertex]
    es: list[ABEdge]


def get_ab_graphx(graph: nx.Graph) -> ABGraph:
    """Convert an networkx graph to an AutoBridge graph."""
    vertex_map = {v: get_ab_vertexx(v, graph) for v in graph.nodes}
    edges = [
        ABEdge(
            source_vertex=vertex_map[src],
            target_vertex=vertex_map[tgt],
            index=idx,
            width=graph.get_edge_data(src, tgt)["width"],
        )
        for idx, (src, tgt) in enumerate(graph.edges)
    ]
    return ABGraph(vs=list(vertex_map.values()), es=edges)


def get_ab_vertexx(v: int | str, g: nx.Graph) -> ABVertex:
    """Convert an networkx vertex to an AutoBridge vertex."""
    return ABVertex(
        name=g.nodes[v]["name"],
        sub_cells=tuple(g.nodes[v]["sub_cells"]),
        area=convert_area(g.nodes[v]["area"]),
        target_slot=g.nodes[v]["target_slot"],
        reserved_slot=g.nodes[v]["reserved_slot"],
    )


def convert_area(area_dict: dict[str, float] | dict[str, int]) -> Area:
    """Convert a dictionary to an Area object."""
    assert all(k in RESOURCES for k in area_dict)

    return Area(
        lut=int(area_dict["LUT"]),
        ff=int(area_dict["FF"]),
        bram_18k=int(area_dict["BRAM_18K"]),
        dsp=int(area_dict["DSP"]),
        uram=int(area_dict["URAM"]),
    )
