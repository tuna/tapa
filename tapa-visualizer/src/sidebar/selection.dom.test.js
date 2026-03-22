// @vitest-environment jsdom

import { within } from "@testing-library/dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mountSidebarDom = () => {
  document.body.innerHTML = `
    <div class="sidebar-content-explorer"></div>
    <div class="sidebar-content-cflags"></div>
    <div class="sidebar-content-instance"></div>
    <div class="sidebar-content-task"></div>
    <div class="sidebar-content-neighbors"></div>
    <div class="sidebar-content-connections"></div>
    <dialog><h2>Code</h2><pre><code></code></pre></dialog>
  `;
};

let selection;

beforeEach(async () => {
  mountSidebarDom();
  vi.resetModules();
  selection = await import("./selection.js");
});

describe("sidebar explorer renderer", () => {
  it("renders explorer items and cflags into the sidebar", () => {
    const state = {
      graph: { setElementState: vi.fn() },
      graphData: { nodes: [], edges: [], combos: [] },
    };
    const controller = selection.createSidebarController(state);

    controller.updateExplorer({
      top: "top",
      cflags: ["-I", "include", "-isystem", "sys"],
      tasks: {
        top: {
          level: "upper",
          target: "xilinx",
          vendor: "amd",
          tasks: {
            child: [{ args: {}, step: 0 }],
          },
          fifos: {},
          ports: [],
          code: "",
        },
        child: {
          level: "lower",
          target: "xilinx",
          vendor: "amd",
          code: "",
        },
      },
    });

    const explorer = document.querySelector(".sidebar-content-explorer");
    const cflags = document.querySelector(".sidebar-content-cflags");

    const explorerItems = within(explorer).getAllByRole("listitem");
    const cflagItems = within(cflags).getAllByRole("listitem");

    expect(explorerItems[0].textContent).toBe("top");
    expect(explorerItems[1].textContent).toBe("  child");
    expect(cflagItems[0].textContent).toBe("-I");
    expect(cflagItems[1].textContent).toBe("include");
    expect(cflagItems[2].textContent).toBe("-isystem sys");
    expect(cflagItems[2].className).toBe("isystem");
  });
});
