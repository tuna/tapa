/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { getComboName } from "../helper.js";

/** @type {(graphData: GraphData) => void} */
export const expandSubTask = graphData => {
  const insertIndex = (id, i) => id.split("/").toSpliced(2, 0, i).join("/");

  /** @type {import("@antv/g6").ComboData[]} */
  const expandedCombos = [];
  /** @type {string[]} */
  const expandedComboIds = [];

  for (let i = 1; i < graphData.combos.length; i++) {
    const combo = graphData.combos[i];
    const comboNodes = graphData.nodes.filter(
      node => node.id.startsWith(`${getComboName(combo.id)}/`)
    );
    if (comboNodes.length <= 1) continue;

    expandedCombos.push(combo);
    expandedComboIds.push(combo.id);

    const children = graphData.nodes.filter(node => node.combo === combo.id);

    for (let j = 1; j < comboNodes.length; j++) {
      const comboId = `${combo.id}/${j}`;
      const newCombo = { ...combo, id: comboId };
      expandedCombos.push(newCombo);
      graphData.combos.push(newCombo);
      graphData.nodes.push(
        ...children.map(node => ({ ...node, id: insertIndex(node.id, j.toString()), combo: comboId })),
      );
    }

    combo.id += "/0";
    children.forEach(node => {
      node.id = insertIndex(node.id, "0");
      node.combo = combo.id;
    });
  }

  expandedCombos.forEach(combo => {
    if (combo.combo && expandedComboIds.includes(combo.combo)) {
      combo.combo += `/${combo.id.split("/").at(-1)}`;
    }
  });
};
