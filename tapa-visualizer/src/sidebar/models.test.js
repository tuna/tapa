import { describe, expect, it } from "vitest";

import {
  getComboSidebarModel,
  getEdgeSidebarModel,
  getNodeSidebarModel,
} from "./models.js";

describe("sidebar models", () => {
  it("builds node sidebar neighbor and connection lists", () => {
    const node = {
      id: "node-a",
      combo: "combo:top",
      data: {
        task: { level: "lower", target: "hw", vendor: "x", code: "" },
      },
    };
    const sources = [{ id: "edge-out", target: "node-b" }];
    const targets = [{ id: "edge-in", source: "node-c" }];

    expect(getNodeSidebarModel(node, sources, targets)).toMatchObject({
      neighbors: ["node-b", "node-c"],
      sources: ["edge-out -> node-b"],
      targets: ["edge-in <- node-c"],
      task: node.data.task,
    });
  });

  it("builds combo sidebar child ids", () => {
    expect(getComboSidebarModel(
      { id: "combo:top" },
      [{ id: "child-a" }, { id: undefined }, { id: "child-b" }],
    )).toEqual({
      childIds: ["child-a", "child-b"],
      taskId: "top",
    });
  });

  it("builds edge sidebar task references", () => {
    const task = { level: "lower", target: "hw", vendor: "x", code: "" };
    expect(getEdgeSidebarModel(
      { id: "edge-a", source: "src", target: "dst" },
      { id: "src", data: { task } },
      { id: "dst", data: { task } },
    )).toMatchObject({
      id: "edge-a",
      source: "src",
      target: "dst",
      sourceTask: task,
      sourceTaskId: "src",
      targetTask: task,
      targetTaskId: "dst",
    });
  });
});
