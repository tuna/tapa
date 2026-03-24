/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

/** @type {Placements} */
const placements = {
  x: [
    [0.5],
    [0.25, 0.75],
    [0, 0.5, 1],
    [0, 0.25, 0.75, 1],
    [0, 0.25, 0.5, 0.75, 1],
  ],
  // y:
  istream: 0,
  ostream: 1,
};

/** subTask -> ioPorts
 * @type {(subTask: SubTask, ioPorts: IOPorts) => void} */
export const setIOPorts = (subTask, ioPorts) => {
  for (const argName in subTask.args) {
    const { arg, cat } = subTask.args[argName];
    if (cat === "istream") ioPorts.istream.push([argName, arg]);
    else if (cat === "ostream") ioPorts.ostream.push([argName, arg]);
  }
};

/** ioPorts -> style.ports
 * @type {(ioPorts: IOPorts, style: NodeStyle) => void} */
export const setPortsStyle = (ioPorts, style) => {
  /** @type {import("@antv/g6").NodePortStyleProps[]} */
  const ports = [];
  for (const stream of /** @type {["istream", "ostream"]} */ (["istream", "ostream"])) {
    const amount = ioPorts[stream].length;
    if (amount <= placements.x.length) {
      ioPorts[stream].forEach(([name, key], i) => {
        /** @type {import("@antv/g6").Placement} */
        const placement = [placements.x[amount - 1][i], placements[stream]];
        ports.push({ name, key, placement });
      });
    }
  }
  style.ports = ports;
}
