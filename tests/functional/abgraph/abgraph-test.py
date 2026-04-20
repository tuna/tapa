# ruff: noqa: INP001

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""
import json

import pytest
from python.runfiles import Runfiles  # type: ignore[reportMissingImports]

_TESTDATA_PATH = "_main/tests/functional/abgraph/{}-abgraph-json.json"
GOLDEN_PATH = "_main/tests/functional/abgraph/golden/{}.json"


def _normalize_abgraph(graph: dict) -> dict:
    """Match the legacy ABGraph equality semantics without importing it."""
    return {
        "vs": sorted(vertex["name"] for vertex in graph["vs"]),
        "es": sorted(
            (
                edge["index"],
                edge["width"],
                edge["source_vertex"]["name"],
                edge["target_vertex"]["name"],
            )
            for edge in graph["es"]
        ),
    }


def test_abgraph(request: pytest.FixtureRequest) -> None:
    """Test if the generated ABGraph matches the golden."""
    test_name = request.config.getoption("--test")

    # Access bazel runfiles
    runfiles = Runfiles.Create()
    assert runfiles is not None
    abgraph_path = runfiles.Rlocation(_TESTDATA_PATH.format(test_name))
    assert abgraph_path is not None
    golden_abgraph = runfiles.Rlocation(GOLDEN_PATH.format(test_name))
    assert golden_abgraph is not None

    with open(abgraph_path, encoding="utf-8") as f:
        abgraph = json.load(f)
    with open(golden_abgraph, encoding="utf-8") as f:
        golden = json.load(f)
    assert _normalize_abgraph(abgraph) == _normalize_abgraph(golden)
