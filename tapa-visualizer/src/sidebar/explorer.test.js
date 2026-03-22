import { describe, expect, it } from "vitest";

import { getExplorerItems, getExplorerSelectionStates } from "./explorer.js";

describe("explorer helpers", () => {
  it("builds nested explorer items", () => {
    const graphJSON = {
      top: "top",
      cflags: [],
      tasks: {
        top: {
          level: "upper",
          target: "x",
          vendor: "y",
          tasks: { child: [{ args: {}, step: 0 }] },
          fifos: {},
          ports: [],
          code: "",
        },
        child: {
          level: "lower",
          target: "x",
          vendor: "y",
          code: "",
        },
      },
    };

    expect(getExplorerItems(graphJSON)).toEqual(["top", "  child"]);
  });

  it("marks the selected element in state map", () => {
    const states = getExplorerSelectionStates(
      {
        nodes: [{ id: "top" }],
        edges: [],
        combos: [{ id: "combo-top" }],
      },
      "top",
    );

    expect(states.top).toEqual(["selected"]);
    expect(states["combo-top"]).toEqual([]);
  });
});
