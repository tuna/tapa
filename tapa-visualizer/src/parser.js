/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { setIOPorts, setPortsStyle } from "./parser/ports.js";
import { expandSubTask } from "./parser/expand.js";
import { parseFifo } from "./parser/fifo.js";

import { color } from "./graph-config.js";
import { getComboId } from "./helper.js";

const altNodeColor = color.nodeB;

/** @type {Readonly<Required<GetGraphDataOptions>>} */
const defaultOptions = {
  grouping: "merge",
  expand: false,
  port: false,
};

/** @type {(json: GraphJSON, options: Required<GetGraphDataOptions>) => GraphData} */
export const getGraphData = (json, options = defaultOptions) => {
  const { grouping, port: showPorts } = options;
  const collapsed = !options.expand;

  /** @type {GraphData} */
  const graphData = {
    nodes: [],
    edges: [],
    combos: [],
  };

  const { nodes, edges, combos } = graphData;

  /** @type {Map<string, UpperTask>} */
  const upperTasks = new Map();
  for (const taskName in json.tasks) {
    const task = json.tasks[taskName];
    if (task.level === "upper") upperTasks.set(taskName, task);
  }

  const colorByTaskLevel = upperTasks.size > 1;

  /** @type {(edge: import("@antv/g6").EdgeData) => void} */
  const addEdge = colorByTaskLevel
    ? edge => edges.push(edge)
    : (() => {
      /** Connection counts: 0 = first seen, 1 = already colored
       * @type {Map<string, 0 | 1>} */
      const counts = new Map();
      return edge => {
        [edge.source, edge.target].forEach(id => {
          switch (counts.get(id)) {
            case undefined: counts.set(id, 0); break;
            case 0: {
              const node = nodes.find(n => n.id === id);
              if (node) node.style = { ...node.style, fill: altNodeColor };
              counts.set(id, 1);
              break;
            }
          }
        });
        edges.push(edge);
      };
    })();

  const topTaskName = json.top;
  const topTask = json.tasks[topTaskName];
  if (!topTask) return graphData;
  if (topTask.level === "lower") {
    nodes.push({ id: topTaskName, data: { task: topTask } });
    return graphData;
  }

  combos.push({ id: getComboId(topTaskName), data: topTask });

  // Loop 1: sub-tasks -> combos and nodes
  upperTasks.forEach((upperTask, upperTaskName) => {
    for (const subTaskName in upperTask.tasks) {
      const subTasks = upperTask.tasks[subTaskName];
      const task = json.tasks[subTaskName];
      const combo = getComboId(upperTaskName);
      // ports don't reset by themselves; always set to override existing ones
      const style = /** @type {NonNullable<NodeData["style"]>} */ ({ ports: [] });

      if (task?.level === "upper") {
        const newCombo = { id: getComboId(subTaskName), combo, data: task, style: { collapsed } };
        const i = combos.findIndex(({ id }) => id === combo);
        i !== -1 ? combos.splice(i + 1, 0, newCombo) : combos.push(newCombo);
        if (colorByTaskLevel) style.fill = altNodeColor;
      }

      if (grouping !== "merge") {
        subTasks.forEach((subTask, i) => {
          if (showPorts) {
            const ioPorts = { istream: [], ostream: [] };
            setIOPorts(subTask, ioPorts);
            setPortsStyle(ioPorts, style);
          }
          nodes.push({ id: `${subTaskName}/${i}`, combo, style, data: { task, subTask } });
        });
      } else {
        if (showPorts) {
          const ioPorts = { istream: [], ostream: [] };
          subTasks.forEach(subTask => setIOPorts(subTask, ioPorts));
          setPortsStyle(ioPorts, style);
        }
        nodes.push({ id: subTaskName, combo, style, data: { task, subTasks } });
      }
    }
  });

  if (grouping === "expand" && graphData.combos.length > 1) {
    expandSubTask(graphData);
  }

  // Loop 2: fifos -> edges
  upperTasks.forEach((upperTask, upperTaskName) =>
    parseFifo(upperTask, upperTaskName, grouping, nodes, addEdge),
  );

  return graphData;
};
