/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

/** @param {import("@antv/g6").Graph} graph
 * @param {() => void} clearGraph
 * @param {() => string | undefined} getFilename
 * @returns {[string, EventListenerOrEventListenerObject][]} */
const getGraphButtons = (graph, clearGraph, getFilename) => [
  [".btn-clearGraph", () => void graph.clear().then(clearGraph)],
  [".btn-rerenderGraph", () => void graph.layout().then(() => graph.fitView())],
  [".btn-fitCenter", () => void graph.fitCenter()],
  [".btn-fitView", () => void graph.fitView()],
  [".btn-saveImage", () => void graph.toDataURL({ mode: "overall" }).then(
    href => Object.assign(document.createElement("a"), { href, download: getFilename(), rel: "noopener" }).click(),
  )],
];

/** @param {import("@antv/g6").Graph} graph
 * @param {() => void} clearGraph
 * @param {() => string | undefined} getFilename */
export const setupGraphButtons = (graph, clearGraph, getFilename) => {
  getGraphButtons(graph, clearGraph, getFilename).forEach(([selector, callback]) => {
    const button = document.querySelector(selector);
    if (button) { button.addEventListener("click", callback); button.disabled = false; }
    else console.warn(`setButton(): "${selector}" don't match any element!`);
  });
};

export const setupSidebarToggle = () => {
  const sidebar = document.querySelector("aside");
  /** @type { HTMLButtonElement | null } */
  const toggleSidebar = document.querySelector(".btn-toggleSidebar");
  if (sidebar && toggleSidebar) {
    toggleSidebar.addEventListener("click", () => {
      const newValue = sidebar.style.display !== "none" ? "none" : null;
      sidebar.style.setProperty("display", newValue);
    });
    toggleSidebar.disabled = false;
  }
};

export const setupCodeDialog = () => {
  const dialog = document.querySelector("dialog");
  if (!dialog) return;

  const closeBtn = dialog.querySelector(":scope .btn-close");
  closeBtn?.addEventListener("click", () => dialog.close());

  const code = dialog.querySelector(":scope > pre > code");
  if (!code) return;

  const copyBtn = dialog.querySelector(":scope .btn-copy");
  copyBtn?.addEventListener(
    "click",
    () => code.textContent &&
      void navigator.clipboard.writeText(code.textContent),
  );
};
