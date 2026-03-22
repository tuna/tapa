import { describe, expect, it } from "vitest";

import { getEmptyGraphData, resetGraphLoaderState } from "./graph-loader.js";

describe("graph loader state", () => {
  it("returns a fresh empty graph data shape", () => {
    expect(getEmptyGraphData()).toEqual({ nodes: [], edges: [], combos: [] });
    expect(getEmptyGraphData()).not.toBe(getEmptyGraphData());
  });

  it("clears stale graph loader state after a failed load", () => {
    const state = {
      filename: "graph.json",
      graph: {},
      graphJSON: { top: "top", tasks: {}, cflags: [] },
      graphData: {
        nodes: [{ id: "node-a" }],
        edges: [{ id: "edge-a" }],
        combos: [{ id: "combo-a" }],
      },
      options: { grouping: "merge", expand: false, port: false },
    };

    resetGraphLoaderState(state);

    expect(state).toMatchObject({
      filename: undefined,
      graphJSON: undefined,
      graphData: { nodes: [], edges: [], combos: [] },
    });
  });
});
