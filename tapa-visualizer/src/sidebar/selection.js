/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { $, $text, append, getComboId } from "../helper.js";
import {
  cflags,
  connections,
  explorer,
  instance,
  neighbors,
  resetSidebar,
  task,
  ul,
} from "./dom.js";
import { getExplorerItems, getExplorerSelectionStates } from "./explorer.js";
import { getComboInfo, getNodeInfo, getTaskInfo } from "./info.js";
import {
  getComboSidebarModel,
  getEdgeSidebarModel,
  getNodeSidebarModel,
} from "./models.js";

const sourcesTitle = append(
  $("p", { textContent: "Sources" }),
  $("br"),
  $("code", {
    className: "hint",
    textContent: "Format: connection name -> target name",
  }),
);
const targetsTitle = append(
  $("p", { textContent: "Targets" }),
  $("br"),
  $("code", {
    className: "hint",
    textContent: "Format: connection name <- source name",
  }),
);

/** @param {{
 *   graph: Graph | undefined,
 *   graphData: GraphData,
 * }} state */
export const createSidebarController = state => {
  const clearExplorer = (message = "Please load a file.") => {
    explorer.replaceChildren($text("p", message));
    cflags.replaceChildren($text("p", message));
  };

  /** @type {(graphJSON: GraphJSON) => void} */
  const updateExplorer = ({ cflags: flags, top, tasks }) => {
    cflags.replaceChildren(
      ul(
        flags.reduce(
          (arr, cur) => {
            const last = arr.length - 1;
            if (cur === "-isystem") {
              arr.push(`${cur} `);
            } else if (arr[last]?.endsWith(" ")) {
              arr[last] += cur;
            } else {
              arr.push(cur);
            }
            return arr;
          },
          /** @type {string[]} */
          ([]),
        ).map(flag => {
          const li = $text("li", flag);
          if (flag.startsWith("-isystem ")) {
            li.className = "isystem";
          }
          return li;
        }),
      ),
    );

    const taskItems = getExplorerItems({ cflags: flags, tasks, top });
    if (taskItems.length === 0) {
      explorer.replaceChildren($text("p", "This graph has no top task."));
      return;
    }

    const taskUl = ul(taskItems.map(item => $text("li", item)));
    taskUl.addEventListener("click", ({ target }) => {
      if (target instanceof HTMLLIElement && target.textContent) {
        const id = target.textContent.trim();
        const states = getExplorerSelectionStates(state.graphData, id);
        if (!(id in states)) {
          const comboId = getComboId(id);
          comboId in states
            ? states[comboId].push("selected")
            : console.warn(`id not found: ${id}`);
        }

        state.graph && void state.graph.setElementState(states);
      }
    });

    explorer.replaceChildren(taskUl);
  };

  /** @param {string} id
   * @param {Graph} graph */
  const updateSidebarForNode = (id, graph) => {
    /** @ts-expect-error @type {NodeData | undefined} */
    const node = graph.getNodeData(id);
    if (!node) {
      resetSidebar(`Node ${id} not found!`);
      return;
    }

    instance.replaceChildren(getNodeInfo(node));

    const taskInfo = node.data.task
      ? getTaskInfo(node.data.task, node.id)
      : [$text("p", "This item has no task infomation.")];
    task.replaceChildren(...taskInfo);

    const sources = graph.getRelatedEdgesData(node.id, "out");
    const targets = graph.getRelatedEdgesData(node.id, "in");
    const sidebarModel = getNodeSidebarModel(node, sources, targets);

    if (sidebarModel.neighbors.length > 0) {
      neighbors.replaceChildren(
        append($("p", { className: "hint" }), $text("code", node.id), "'s neighbors:"),
        ul(sidebarModel.neighbors.map(neighborId => $text("li", neighborId))),
      );
    } else {
      neighbors.replaceChildren($text("p", `Node ${node.id} has no neighbors.`));
    }

    connections.replaceChildren(
      append($("p", { className: "hint" }), $text("code", node.id), "'s connections:"),
      sourcesTitle,
      sidebarModel.sources.length !== 0
        ? ul(sidebarModel.sources.map(connection => $text("li", connection)))
        : $("p", { textContent: "none", style: "padding-inline-start: 1em; font-size: .85rem;" }),
      targetsTitle,
      sidebarModel.targets.length !== 0
        ? ul(sidebarModel.targets.map(connection => $text("li", connection)))
        : $("p", { textContent: "none", style: "padding-inline-start: 1em; font-size: .85rem;" }),
    );
  };

  /** @param {string} id
   * @param {Graph} graph */
  const updateSidebarForCombo = (id, graph) => {
    /** @ts-expect-error @type {ComboData | undefined} */
    const combo = graph.getComboData(id);
    if (!combo) {
      resetSidebar(`Combo ${id} not found!`);
      return;
    }

    const sidebarModel = getComboSidebarModel(combo, graph.getChildrenData(combo.id));
    instance.replaceChildren(getComboInfo(combo, graph));
    task.replaceChildren(...getTaskInfo(combo.data, sidebarModel.taskId));
    neighbors.replaceChildren($text("p", "Please select a node."));
    connections.replaceChildren($text("p", "Please select a node."));
  };

  /** @param {string} id
   * @param {Graph} graph */
  const updateSidebarForEdge = (id, graph) => {
    const edge = graph.getEdgeData(id);
    if (!edge) {
      resetSidebar(`Edge ${id} not found!`);
      return;
    }

    instance.replaceChildren(
      append(
        $("dl"),
        $text("dt", "Edge Name"),
        $text("dd", id),
        $text("dt", "Source Node"),
        $text("dd", edge.source),
        $text("dt", "Target Node"),
        $text("dd", edge.target),
      ),
    );

    /** @type {HTMLElement[]} */
    const taskElements = [];
    /** @ts-expect-error @type {NodeData} */
    const sourceNode = graph.getNodeData(edge.source);
    /** @ts-expect-error @type {NodeData} */
    const targetNode = graph.getNodeData(edge.target);
    const sidebarModel = getEdgeSidebarModel(edge, sourceNode, targetNode);

    sidebarModel.sourceTask && sidebarModel.sourceTaskId &&
      taskElements.push(
        $text("h3", "Source Task"),
        ...getTaskInfo(sidebarModel.sourceTask, sidebarModel.sourceTaskId),
      );

    sidebarModel.sourceTask &&
    sidebarModel.targetTask &&
    taskElements.push($("hr"));

    sidebarModel.targetTask && sidebarModel.targetTaskId &&
      taskElements.push(
        $text("h3", "Target Task"),
        ...getTaskInfo(sidebarModel.targetTask, sidebarModel.targetTaskId),
      );

    if (taskElements.length !== 0) {
      task.replaceChildren(...taskElements);
    } else {
      task.replaceChildren($("p", { style: "opacity: .75;", textContent: "This edge has no task infomation." }));
    }
  };

  return {
    clearExplorer,
    updateExplorer,
    updateSidebarForCombo,
    updateSidebarForEdge,
    updateSidebarForNode,
  };
};
