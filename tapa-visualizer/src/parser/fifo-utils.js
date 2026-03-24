/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { color } from "../graph-config.js";

/** Color for edge connecting parent and children */
export const altEdgeColor = color.edgeB;

/** fifo groups, for fifos like fifo_xx[0], fifo_xx[1]...
 * @type {Map<string, Set<number>>} */
export const fifoGroups = new Map();

/** @type {(fifoName: string, source: string, target: string, id: string) => boolean} */
export const matchFifoGroup = (fifoName, source, target, id) => {
  const matchResult = fifoName.match(/^(.*)\[(\d+)\]$/);
  const matched = matchResult !== null;
  if (matched) {
    const name = matchResult[1];
    const idSegments = id.replace(`/${fifoName}`, "");
    const key = [name, source, target, idSegments].join("\n");
    const index = Number.parseInt(matchResult[2]);

    const set = fifoGroups.get(key);
    set ? set.add(index) : fifoGroups.set(key, new Set([index]));
  }
  return matched;
};

/** improve the formating of fifo groups: [1,2,3,5] -> "1~3,5"
 * @type {(indexArr: number[]) => string} */
export const getIndexRange = indexes => {
  if (indexes.length === 1) return `${indexes[0]}`;

  indexes.sort((a, b) => a - b);

  const groups = [[indexes[0]]];
  for (let i = 1; i < indexes.length; i++) {
    indexes[i - 1] + 1 === indexes[i]
      ? groups.at(-1)?.push(indexes[i])
      : groups.push([indexes[i]]);
  }

  return groups.map(
    group => group.length === 1 ? group[0] : `${group[0]}~${group.at(-1)}`,
  ).join(",");
};

/** get port's key for missing produced_by or consumed_by
 * @type {(node: import("@antv/g6").NodeData | undefined, fifoName: string) => string} */
export const getPortKey = (node, fifoName) => node?.style?.ports
  ?.find(port => "name" in port && port.name === fifoName)?.key ?? fifoName;

/** @type {(by: [string, number], grouping: Grouping) => string} */
const getSubTaskKey = (by, grouping) => grouping !== "merge"
  ? `${by[0]}/${by[1]}`
  : by[0];

/** @type {(node: import("@antv/g6").NodeData | undefined, fifoName: string) => import("@antv/g6/lib/spec/element/edge").EdgeStyle} */
const getProducedMissingStyle = (node, fifoName) => ({
  sourcePort: getPortKey(node, fifoName),
  targetPort: fifoName,
  stroke: altEdgeColor,
});

/** @type {(node: import("@antv/g6").NodeData | undefined, fifoName: string) => import("@antv/g6/lib/spec/element/edge").EdgeStyle} */
const getConsumedMissingStyle = (node, fifoName) => ({
  sourcePort: fifoName,
  targetPort: getPortKey(node, fifoName),
  stroke: altEdgeColor,
});

/** @type {(params: {
 *   fifoName: string,
 *   source: string,
 *   target: string,
 *   id: string,
 *   fifo: UpperTask["fifos"][string],
 *   addEdge: (edge: import("@antv/g6").EdgeData) => void,
 *   getStyle: () => import("@antv/g6/lib/spec/element/edge").EdgeStyle,
 * }) => void} */
const addFifoEdge = ({ addEdge, fifo, fifoName, getStyle, id, source, target }) => {
  if (!matchFifoGroup(fifoName, source, target, id)) {
    addEdge({ source, target, id, style: getStyle(), data: { fifo } });
  }
};

/** @type {(params: {
 *   fifoName: string,
 *   fifo: UpperTask["fifos"][string],
 *   taskName: string,
 *   grouping: Grouping,
 *   nodes: GraphData["nodes"],
 *   addEdge: (edge: import("@antv/g6").EdgeData) => void,
 * }) => void} */
const addConnectedFifo = ({
  addEdge,
  fifo,
  fifoName,
  grouping,
  taskName,
}) => {
  const source = getSubTaskKey(fifo.produced_by, grouping);
  const target = getSubTaskKey(fifo.consumed_by, grouping);
  const style = { sourcePort: fifoName, targetPort: fifoName };
  addFifoEdge({
    addEdge,
    fifo,
    fifoName,
    getStyle: () => style,
    id: `${taskName}/${fifoName}`,
    source,
    target,
  });
};

/** @type {(params: {
 *   fifoName: string,
 *   fifo: UpperTask["fifos"][string],
 *   taskName: string,
 *   grouping: Grouping,
 *   nodes: GraphData["nodes"],
 *   addEdge: (edge: import("@antv/g6").EdgeData) => void,
 * }) => void} */
const addProducedMissingFifo = ({
  addEdge,
  fifo,
  fifoName,
  grouping,
  nodes,
  taskName,
}) => {
  /** @type {(node: import("@antv/g6").NodeData | undefined) => () => import("@antv/g6/lib/spec/element/edge").EdgeStyle} */
  const styleFor = node => () => getProducedMissingStyle(node, fifoName);
  switch (grouping) {
    case "merge": {
      const node = /** @type {import("@antv/g6").NodeData | undefined} */ (
        nodes.find(node => node.id === taskName)
      );
      addFifoEdge({
        addEdge,
        fifo,
        fifoName,
        getStyle: styleFor(node),
        id: `${taskName}/${fifoName}`,
        source: taskName,
        target: fifo.consumed_by[0],
      });
      break;
    }
    case "separate": {
      const target = fifo.consumed_by.join("/");
      nodes
        .filter(node => node.id.startsWith(`${taskName}/`))
        .forEach(node => {
          const edgeId = `${taskName}/${fifoName}${node.id.slice(node.id.indexOf("/"))}`;
          addFifoEdge({
            addEdge,
            fifo,
            fifoName,
            getStyle: styleFor(node),
            id: edgeId,
            source: node.id,
            target,
          });
        });
      break;
    }
    case "expand": {
      const targetId = fifo.consumed_by.join("/");
      const sources = nodes.filter(({ id }) => id.startsWith(`${taskName}/`));
      const targets = nodes.filter(
        ({ id }) => id === targetId || id.startsWith(`${targetId}/`),
      );
      const len = Math.min(sources.length, targets.length);
      for (let i = 0; i < len; i++) {
        const source = sources[i];
        addFifoEdge({
          addEdge,
          fifo,
          fifoName,
          getStyle: styleFor(source),
          id: `${taskName}/${fifoName}/${i}`,
          source: source.id,
          target: targets[i].id,
        });
      }
      break;
    }
  }
};

/** @type {(params: {
 *   fifoName: string,
 *   fifo: UpperTask["fifos"][string],
 *   taskName: string,
 *   grouping: Grouping,
 *   nodes: GraphData["nodes"],
 *   addEdge: (edge: import("@antv/g6").EdgeData) => void,
 * }) => void} */
const addConsumedMissingFifo = ({
  addEdge,
  fifo,
  fifoName,
  grouping,
  nodes,
  taskName,
}) => {
  /** @type {(node: import("@antv/g6").NodeData | undefined) => () => import("@antv/g6/lib/spec/element/edge").EdgeStyle} */
  const styleFor = node => () => getConsumedMissingStyle(node, fifoName);
  switch (grouping) {
    case "merge": {
      const node = /** @type {import("@antv/g6").NodeData | undefined} */ (
        nodes.find(node => node.id === taskName)
      );
      addFifoEdge({
        addEdge,
        fifo,
        fifoName,
        getStyle: styleFor(node),
        id: `${taskName}/${fifoName}`,
        source: fifo.produced_by[0],
        target: taskName,
      });
      break;
    }
    case "separate": {
      const source = fifo.produced_by.join("/");
      nodes
        .filter(node => node.id.startsWith(`${taskName}/`))
        .forEach(node => {
          const edgeId = `${taskName}/${fifoName}${node.id.slice(node.id.indexOf("/"))}`;
          addFifoEdge({
            addEdge,
            fifo,
            fifoName,
            getStyle: styleFor(node),
            id: edgeId,
            source,
            target: node.id,
          });
        });
      break;
    }
    case "expand": {
      const sourceId = fifo.produced_by.join("/");
      const sources = nodes.filter(
        ({ id }) => id === sourceId || id.startsWith(`${sourceId}/`),
      );
      const targets = nodes.filter(({ id }) => id.startsWith(`${taskName}/`));
      const len = Math.min(sources.length, targets.length);
      for (let i = 0; i < len; i++) {
        const target = targets[i];
        addFifoEdge({
          addEdge,
          fifo,
          fifoName,
          getStyle: styleFor(target),
          id: `${taskName}/${fifoName}/${i}`,
          source: sources[i].id,
          target: target.id,
        });
      }
      break;
    }
  }
};

/** @type {(params: {
 *   fifoName: string,
 *   fifo: UpperTask["fifos"][string],
 *   taskName: string,
 *   grouping: Grouping,
 *   nodes: GraphData["nodes"],
 *   addEdge: (edge: import("@antv/g6").EdgeData) => void,
 * }) => void} */
export const addFifo = params => {
  const { fifo, fifoName, taskName } = params;
  if (fifo.produced_by && fifo.consumed_by) {
    addConnectedFifo(params);
  } else if (!fifo.produced_by && fifo.consumed_by) {
    addProducedMissingFifo(params);
  } else if (fifo.produced_by && !fifo.consumed_by) {
    addConsumedMissingFifo(params);
  } else {
    console.warn(
      `fifo ${fifoName} without produced_by and consumed_by in ${taskName}:`,
      params,
    );
  }
};
