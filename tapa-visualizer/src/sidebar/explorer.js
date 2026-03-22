/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

/** @param {GraphJSON} graphJSON
 * @returns {string[]} */
export const getExplorerItems = ({ tasks, top }) => {
  /** @type {string[]} */
  const items = [];

  /** @param {UpperTask} task
   * @param {number} level */
  const parseUpperTask = (task, level) => {
    for (const subTaskName in task.tasks) {
      const subTask = tasks[subTaskName];
      if (!subTask) {
        console.warn(`task not found: ${subTaskName}`);
        continue;
      }
      items.push(`${"  ".repeat(level)}${subTaskName}`);
      if (subTask.level === "upper") {
        parseUpperTask(subTask, level + 1);
      }
    }
  };

  const topTask = tasks[top];
  if (!topTask) {
    return [];
  }

  items.push(top);
  if (topTask.level === "upper") {
    parseUpperTask(topTask, 1);
  }
  return items;
};

/** @param {GraphData} graphData
 * @param {string} selectedId
 * @returns {Record<string, string[]>} */
export const getExplorerSelectionStates = (graphData, selectedId) => {
  /** @type {Record<string, string[]>} */
  const states = {};
  [graphData.nodes, graphData.edges, graphData.combos].forEach(
    items => items.forEach(({ id }) => { states[id ?? ""] = []; }),
  );

  if (selectedId in states) {
    states[selectedId].push("selected");
  }
  return states;
};
