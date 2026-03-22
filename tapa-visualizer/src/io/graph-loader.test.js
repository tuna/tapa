// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";

vi.mock("../sidebar.js", () => ({
  resetInstance: vi.fn(),
  resetSidebar: vi.fn(),
}));
vi.mock("../graph-config.js", () => ({
  color: { edgeB: "#000000" },
}));

import { resetSidebar } from "../sidebar.js";
import { getEmptyGraphData, resetGraphLoaderState, setupGraphLoader } from "./graph-loader.js";

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

  it("resets the full loader path when parsing fails", async () => {
    const state = {
      filename: undefined,
      graph: { clear: vi.fn(() => Promise.resolve()) },
      graphJSON: undefined,
      graphData: getEmptyGraphData(),
      options: { grouping: "merge", expand: false, port: false },
    };
    const deps = {
      state,
      getOptions: vi.fn(() => state.options),
      renderGraph: vi.fn(),
      setupGraph: vi.fn(),
      clearExplorer: vi.fn(),
      updateExplorer: vi.fn(),
      updateOptionsHint: vi.fn(),
    };
    const file = {
      name: "graph.json",
      type: "application/json",
      text: vi.fn(() => Promise.resolve("{")),
    };
    const fileInput = {
      files: [file],
      addEventListener: vi.fn(),
    };
    const flush = () => new Promise(resolve => setTimeout(resolve, 0));

    setupGraphLoader(fileInput, deps);
    await flush();

    expect(deps.updateExplorer).not.toHaveBeenCalled();
    expect(deps.clearExplorer).toHaveBeenCalledTimes(1);
    expect(deps.updateOptionsHint).toHaveBeenCalledWith(0);
    expect(state).toMatchObject({
      filename: undefined,
      graphJSON: undefined,
      graphData: { nodes: [], edges: [], combos: [] },
    });
    expect(state.graph.clear).toHaveBeenCalledTimes(1);
    expect(resetSidebar).toHaveBeenCalledWith("Loading...");
    expect(resetSidebar).toHaveBeenLastCalledWith(
      "TypeError: Invalid graph.json: file is not valid JSON.",
    );
  });
});
