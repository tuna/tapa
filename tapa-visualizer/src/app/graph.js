/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { DragCanvas, Graph } from "@antv/g6";
import { graphOptions } from "../graph-config.js";

/** @param {{
 *   getLayout: () => import("@antv/g6").LayoutOptions,
 *   onResetSidebar: (message?: string) => void,
 *   onComboClick: (id: string, graph: Graph) => void,
 *   onEdgeClick: (id: string, graph: Graph) => void,
 *   onNodeClick: (id: string, graph: Graph) => void,
 * }} deps */
export const createGraph = ({
  getLayout,
  onComboClick,
  onEdgeClick,
  onNodeClick,
  onResetSidebar,
}) => {
  /** @type {(states: Record<string, string[]>) => Record<string, string[]>} */
  const showSelectedNodes = states => {
    const selected = Object.keys(states);
    if (selected.length > 0) onResetSidebar(`Selected nodes: ${selected.join(", ")}`);
    return states;
  };

  /** @type {Graph} */
  const graph = new Graph({
    ...graphOptions,
    layout: getLayout(),
    behaviors: [
      "zoom-canvas",
      "drag-element",
      /** drag canvas when Shift or Ctrl are not pressed
       * @type {import("@antv/g6").DragCanvasOptions} */
      ({
        type: "drag-canvas",
        enable: event => {
          if (event.ctrlKey || event.shiftKey) return false;
          const defaultEnable = DragCanvas.defaultOptions.enable;
          return typeof defaultEnable === "function" ? defaultEnable(event) : true;
        },
      }),
      /** Shift + drag: brush select (box selection)
       * @type {import("@antv/g6").BrushSelectOptions} */
      ({
        type: "brush-select",
        trigger: ["shift"],
        mode: "diff",
        enableElements: ["node"],
        onSelect: showSelectedNodes,
      }),
      /** Ctrl + drag: lasso select
       * @type {import("@antv/g6").LassoSelectOptions} */
      ({
        type: "lasso-select",
        trigger: ["control"],
        mode: "diff",
        enableElements: ["node"],
        onSelect: showSelectedNodes,
      }),
      /** Double click to collapse / expand combo
       * @type {import("@antv/g6").CollapseExpandOptions} */
      ({
        type: "collapse-expand",
        animation: false,
        onExpand: id =>
          graph.getComboData(id) &&
          graph.getChildrenData(id).length > 1 &&
          void graph.layout(),
      }),
      /** @type {import("@antv/g6").ClickSelectOptions} */
      ({
        type: "click-select",
        degree: 1,
        neighborState: "highlight",
        onClick: ({ target: item }) => {
          if (!("type" in item) || !("id" in item)) { onResetSidebar(); return; }
          switch (item.type) {
            case "node": onNodeClick(item.id, graph); break;
            case "combo": onComboClick(item.id, graph); break;
            case "edge": onEdgeClick(item.id, graph); break;
            default: onResetSidebar(); break;
          }
        },
      }),
    ],
  });
  return graph;
};
