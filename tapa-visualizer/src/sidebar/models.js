/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { getComboName } from "../helper.js";

/** @param {NodeData} node
 * @param {import("@antv/g6").EdgeData[]} sources
 * @param {import("@antv/g6").EdgeData[]} targets */
export const getNodeSidebarModel = (node, sources, targets) => {
  /** @type {string[]} */
  const neighbors = [];
  const neighborIds = new Set();
  sources.forEach(edge => neighborIds.add(edge.target));
  targets.forEach(edge => neighborIds.add(edge.source));
  neighbors.push(...neighborIds.values());

  return {
    neighbors,
    sources: sources.map(edge => `${edge.id} -> ${edge.target}`),
    targets: targets.map(edge => `${edge.id} <- ${edge.source}`),
    task: node.data.task,
  };
};

/** @param {ComboData} combo
 * @param {{ id?: string }[]} children */
export const getComboSidebarModel = (combo, children) => ({
  childIds: children
    .map(child => child.id)
    .filter(id => typeof id === "string"),
  taskId: getComboName(combo.id),
});

/** @param {import("@antv/g6").EdgeData} edge
 * @param {NodeData | undefined} sourceNode
 * @param {NodeData | undefined} targetNode */
export const getEdgeSidebarModel = (edge, sourceNode, targetNode) => ({
  id: edge.id,
  source: edge.source,
  target: edge.target,
  sourceTask: sourceNode?.data?.task,
  sourceTaskId: sourceNode?.id,
  targetTask: targetNode?.data?.task,
  targetTaskId: targetNode?.id,
});
