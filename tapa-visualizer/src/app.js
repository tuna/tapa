/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { createIcons, icons } from "lucide";

createIcons({ icons });

import { setupFileInput } from "./app/file-loader.js";
import { createGraph } from "./app/graph.js";
import { setupCodeDialog, setupGraphButtons, setupSidebarToggle } from "./app/ui.js";
import { antvDagre, dagre, forceAtlas2 } from "./graph-config.js";
import { getEmptyGraphData } from "./io/graph-loader.js";
import {
  createSidebarController,
  resetInstance,
  resetSidebar,
} from "./sidebar.js";
import { getComboId } from "./helper.js";
import { getGraphData } from "./parser.js";

import "../css/style.css";

/** @type {{
 *   filename: string | undefined,
 *   graph: Graph | undefined,
 *   graphJSON: GraphJSON | undefined,
 *   graphData: GraphData,
 *   options: GetGraphDataOptions,
 * }} */
const visualizerState = {
  filename: undefined,
  graph: undefined,
  graphJSON: undefined,
  graphData: getEmptyGraphData(),
  options: { grouping: "merge", expand: false, port: false },
};

/** @satisfies {GroupingForm | null} */
const groupingForm = document.querySelector(".grouping");

/** @satisfies {OptionsForm | null} */
const optionsForm = document.querySelector(".sidebar-content-options");

const getLayout = (layout = optionsForm?.elements.layout.value) => {
  switch (layout) {
    case "force-atlas2": return forceAtlas2;
    case "antv-dagre": return antvDagre;
    case "dagre": return dagre;
    default: return forceAtlas2;
  }
};

const getOptions = () => ({
  grouping: groupingForm?.elements.grouping.value ?? "merge",
  expand: optionsForm?.elements.expand.value === "true",
  port: optionsForm?.elements.port.value === "true",
});

const options = getOptions();
visualizerState.options = options;

/** @type {(graph: Graph, graphData: GraphData) => Promise<void>} */
const renderGraph = async (graph, graphData) => {
  antvDagre.sortByCombo = graphData.combos.length > 1;
  if (graph.rendered && graph.getZoom() !== 1) {
    graph.setData({});
    await graph.draw();
    await graph.zoomTo(1, false);
  }
  graph.setData(graphData);
  await graph.render();
};

/** @param {Graph} graph
 * @param {GraphJSON} graphJSON */
const setupGraph = async (graph, graphJSON) => {
  const topChildren = graph.getChildrenData(getComboId(graphJSON.top));
  const visibleElements = options.expand ? visualizerState.graphData.nodes : topChildren;

  // Tune kg for force-atlas2 based on visible element count
  const layout = graph.getLayout();
  if (!Array.isArray(layout) && layout.type === "force-atlas2") {
    const kg = visibleElements.length >= 25 ? 10 : 1;
    forceAtlas2.kg = kg;
    graph.setLayout(prev => ({ ...prev, kg }));
    await graph.layout();
  }

  if (visibleElements.length >= 10 && visibleElements.length <= 500) {
    await graph.layout(getLayout());
  }

  // Put edges in front of nodes
  await graph.frontElement(
    graph.getEdgeData().map(({ id }) => id).filter(id => typeof id === "string")
  );

  // Run translateElementTo() twice to reset position for collapsed combo
  if (!options.expand) {
    topChildren.forEach(item => {
      if (item.type === "circle" && item.style?.collapsed) {
        const position = graph.getElementPosition(item.id);
        void (async () => {
          await graph.translateElementTo(item.id, position, false);
          await graph.translateElementTo(item.id, position, false);
        })();
      }
    });
  }
};

/** @param {Graph} graph */
const setupRadioToggles = graph => {

  /** @type {(newOption: GetGraphDataOptions) => Promise<void>} */
  const updateGraph = async (newOption) => {
    Object.assign(options, newOption);
    visualizerState.options = options;
    if (!visualizerState.graphJSON) return;
    visualizerState.graphData = getGraphData(visualizerState.graphJSON, options);
    console.debug("graphData:\n", visualizerState.graphData, "\ngetGraphData() options:", options);
    resetSidebar("Loading...");
    await renderGraph(graph, visualizerState.graphData);
    resetInstance();
  };

  if (groupingForm) {
    groupingForm.addEventListener("change",
      () => void updateGraph({ grouping: groupingForm.grouping.value }));
  }

  if (optionsForm) {
    optionsForm.addEventListener("change", ({ target }) => {
      if (!(target instanceof HTMLInputElement)) return;
      if (target.name === "layout") {
        graph.setLayout(getLayout());
        void graph.layout().then(() => graph.fitView());
      } else {
        void updateGraph({ [target.name]: target.value === "true" });
      }
    });
  }
};

(() => {
  /** @satisfies {HTMLInputElement & { files: FileList } | null} */
  const fileInput = document.querySelector("input.fileInput");
  if (fileInput === null) {
    throw new TypeError("Element input.fileInput not found!");
  }

  setupSidebarToggle();
  setupCodeDialog();
  const sidebarController = createSidebarController(visualizerState);

  const graph = createGraph({
    getLayout,
    onResetSidebar: resetSidebar,
    onComboClick: sidebarController.updateSidebarForCombo,
    onEdgeClick: sidebarController.updateSidebarForEdge,
    onNodeClick: sidebarController.updateSidebarForNode,
  });
  visualizerState.graph = graph;

  // Graph loading finished, remove loading status in instance sidebar
  resetInstance();

  setupFileInput(fileInput, {
    state: /** @type {typeof visualizerState & { graph: Graph }} */ (visualizerState),
    clearExplorer: sidebarController.clearExplorer,
    getOptions,
    renderGraph,
    setupGraph,
    updateExplorer: sidebarController.updateExplorer,
    updateOptionsHint: comboCount => {
      const classListMethod = comboCount <= 1 ? "add" : "remove";
      optionsForm?.classList[classListMethod]("only-one-combo");
    },
  });
  setupGraphButtons(
    graph,
    () => {
      visualizerState.filename = undefined;
      visualizerState.graphJSON = undefined;
      visualizerState.graphData = getEmptyGraphData();
      sidebarController.clearExplorer();
      resetSidebar("Please load a file.");
    },
    () => visualizerState.filename,
  );
  setupRadioToggles(graph);

  console.debug("graph object:\n", graph);
})();
