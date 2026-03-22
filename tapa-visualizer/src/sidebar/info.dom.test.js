// @vitest-environment jsdom

import { fireEvent, within } from "@testing-library/dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mountSidebarDom = () => {
  document.body.innerHTML = `
    <div class="sidebar-content-explorer"></div>
    <div class="sidebar-content-cflags"></div>
    <div class="sidebar-content-instance"></div>
    <div class="sidebar-content-task"></div>
    <div class="sidebar-content-neighbors"></div>
    <div class="sidebar-content-connections"></div>
    <dialog><h2>Code</h2><pre><code></code></pre><button class="btn-close"></button><button class="btn-copy"></button></dialog>
  `;

  const dialog = /** @type {HTMLDialogElement & { showModal?: () => void }} */ (
    document.querySelector("dialog")
  );
  dialog.showModal = vi.fn();
};

let info;

beforeEach(async () => {
  mountSidebarDom();
  vi.resetModules();
  info = await import("./info.js");
});

describe("sidebar info renderers", () => {
  it("renders node details and sub-task information", () => {
    const nodeInfo = info.getNodeInfo({
      id: "node-a",
      combo: "combo:top",
      data: {
        subTask: {
          args: { input: { arg: "value", cat: "int" } },
          step: 7,
        },
      },
    });

    expect(within(nodeInfo).getByText("Node Name")).toBeTruthy();
    expect(within(nodeInfo).getByText("node-a")).toBeTruthy();
    expect(within(nodeInfo).getByText("Upper Task")).toBeTruthy();
    expect(within(nodeInfo).getByText("top")).toBeTruthy();
    expect(within(nodeInfo).getByText("Arguments")).toBeTruthy();
    expect(within(nodeInfo).getByText("input: value (int)")).toBeTruthy();
    expect(within(nodeInfo).getByText("Step")).toBeTruthy();
    expect(within(nodeInfo).getByText("7")).toBeTruthy();
  });

  it("renders combo details from graph children", () => {
    const comboInfo = info.getComboInfo(
      { id: "combo:top" },
      { getChildrenData: vi.fn(() => [{ id: "child-a" }, { id: "child-b" }]) },
    );

    expect(within(comboInfo).getByText("Combo Name")).toBeTruthy();
    expect(within(comboInfo).getByText("combo:top")).toBeTruthy();
    expect(within(comboInfo).getByText("Children")).toBeTruthy();
    expect(within(comboInfo).getByText("child-a")).toBeTruthy();
    expect(within(comboInfo).getByText("child-b")).toBeTruthy();
  });

  it("renders task code and enables the code dialog", () => {
    const taskInfo = info.getTaskInfo(
      {
        level: "upper",
        target: "xilinx",
        vendor: "amd",
        ports: [],
        fifos: {},
        tasks: {},
        code: "int main() {}",
      },
      "top/task",
    );

    const button = within(taskInfo[0]).getByRole("button", {
      name: "Show C++ Code",
    });

    fireEvent.click(button);

    const dialog = document.querySelector("dialog");
    expect(dialog.querySelector("h2").textContent).toBe("top");
    expect(dialog.querySelector("code").textContent).toBe("int main() {}");
  });
});
