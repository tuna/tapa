/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { fifoGroups, getIndexRange, addFifo } from "./fifo-utils.js";

/** @param {UpperTask} task
 * @param {string} taskName
 * @param {Grouping} grouping
 * @param {GraphData["nodes"]} nodes
 * @param {(edge: import("@antv/g6").EdgeData) => void} addEdge
 **/
export const parseFifo = (task, taskName, grouping, nodes, addEdge) => {
  fifoGroups.clear();

  for (const fifoName in task.fifos) {
    addFifo({
      addEdge,
      fifo: task.fifos[fifoName],
      fifoName,
      grouping,
      nodes,
      taskName,
    });
  }

  fifoGroups.forEach((indexes, key) => {
    const [name, source, target, idSegments] = key.split("\n");
    const segments = idSegments.split("/");
    const prefix = segments.shift();
    const suffix = segments.length > 0
      ? segments.map(segment => `/${segment}`).join("")
      : "";

    const indexRange = getIndexRange([...indexes.values()]);
    const id = `${prefix}/${name}[${indexRange}]${suffix}`;
    addEdge({ source, target, id });
  });
};
