/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { $, $text, append } from "../helper.js";
import Prism from "../prism-config.js";

const getSidebarContainer = name => {
  const element = document.querySelector(`.sidebar-content-${name}`);
  if (element) return element;
  throw new TypeError(`Element .sidebar-content-${name} not found!`);
};

const explorer = getSidebarContainer("explorer");
const cflags = getSidebarContainer("cflags");
const instance = getSidebarContainer("instance");
const task = getSidebarContainer("task");
const neighbors = getSidebarContainer("neighbors");
const connections = getSidebarContainer("connections");

export { cflags, connections, explorer, instance, neighbors, task };

export const resetInstance = (text = "Please select an item.") => instance.replaceChildren($text("p", text));

export const resetSidebar = (instanceText = "Please select an item.") => {
  instance.replaceChildren($text("p", instanceText));
  task.replaceChildren($text("p", "Please select a node or combo."));
  neighbors.replaceChildren($text("p", "Please select a node."));
  connections.replaceChildren($text("p", "Please select a node."));
};

/** @type {(elements: (Node | string)[]) => HTMLUListElement} */
export const ul = elements => append(
  $("ul", { style: "font-family: monospace; white-space: pre-wrap;" }),
  ...elements,
);

/** @type {(args: [string, { arg: string, cat: string }][]) => HTMLElement} */
export const parseArgs = args => append(
  $("dd"), append(
    $("ul"), ...args.map(
      ([name, { arg, cat }]) => $text("li", `${name}: ${arg} (${cat})`),
    ),
  ),
);

/** @type {(fifos: [string, FIFO][], taskName: string) => HTMLTableElement} */
export const parseFifos = (fifos, taskName) => append(
  $("table", { style: "text-align: center;" }),
  append($("tr"), $text("th", "Name"), $text("th", "Source -> Target"), $text("th", "Depth")),
  ...fifos.map(([name, { produced_by, consumed_by, depth }]) => append(
    $("tr"),
    $text("td", name),
    $text("td", `${produced_by?.join("/") ?? taskName} -> ${consumed_by?.join("/") ?? taskName}`),
    $text("td", depth ?? "/"),
  )),
);

/** @type {(ports: Port[]) => HTMLTableElement} */
export const parsePorts = ports => append(
  $("table", { className: "upperTask-ports" }),
  append($("tr"), $text("th", "Name"), $text("th", "Category"), $text("th", "Type"), $text("th", "Width")),
  ...ports.map(({ name, cat, type, width }) =>
    append($("tr"), ...[name, cat, type, width].map(v => $text("td", v)))),
);

const codeDialog = document.querySelector("dialog");
const codeContainer = document.querySelector("dialog code");

/** @type {(code: string, taskName: string) => HTMLButtonElement} */
export const showCode = codeDialog && codeContainer
  ? (code, taskName) => {
    const button = $text("button", "Show C++ Code");
    button.addEventListener("click", () => {
      codeContainer.textContent = code;
      Prism.highlightElement(codeContainer);
      codeContainer[code.length >= 2500 ? "setAttribute" : "removeAttribute"]("style", "font-size: .8rem;");
      const title = codeDialog.querySelector(":scope h2");
      if (title) title.textContent = taskName;
      codeDialog.showModal();
    });
    return button;
  }
  : () => $("button", { textContent: "Show C++ Code", title: "Error: C++ code-related element(s) does not exist!", disabled: true });
