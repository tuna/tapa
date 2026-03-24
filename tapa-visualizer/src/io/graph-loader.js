/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { resetInstance, resetSidebar } from "../sidebar.js";
import { getGraphData } from "../parser.js";
import { parseGraphJSON } from "./schema.js";

export const getEmptyGraphData = () => ({ nodes: [], edges: [], combos: [] });

/** @typedef {{
 *   state: { filename: string | undefined, graph: import("@antv/g6").Graph, graphJSON: GraphJSON | undefined, graphData: GraphData, options: GetGraphDataOptions },
 *   getOptions: () => GetGraphDataOptions,
 *   renderGraph: (graph: import("@antv/g6").Graph, graphData: GraphData) => Promise<void>,
 *   setupGraph: (graph: import("@antv/g6").Graph, graphJSON: GraphJSON) => Promise<void>,
 *   clearExplorer: (message?: string) => void,
 *   updateExplorer: (graphJSON: GraphJSON) => void,
 *   updateOptionsHint: (comboCount: number) => void,
 * }} GraphLoaderDeps
 */

/** @param {GraphLoaderDeps["state"]} state */
export const resetGraphLoaderState = state => {
  state.filename = undefined;
  state.graphJSON = undefined;
  state.graphData = getEmptyGraphData();
};

/** @param {HTMLInputElement & { files: FileList }} fileInput
 * @param {GraphLoaderDeps} deps */
export const setupGraphLoader = (fileInput, deps) => {
  const readFile = () => {
    /** @type {File | undefined} */
    const file = fileInput.files[0];

    if (!file) return false;
    if (file.type !== "application/json") {
      console.warn("File type is not application/json!");
    }

    resetSidebar("Loading...");

    file.text().then(async text => {
      const graphJSON = parseGraphJSON(text);
      deps.updateExplorer(graphJSON);

      const options = deps.getOptions();
      const graphData = getGraphData(graphJSON, options);
      deps.state.filename = file.name;
      deps.state.graphJSON = graphJSON;
      deps.state.graphData = graphData;
      deps.state.options = options;

      console.debug(
        `${file.name}:\n`, graphJSON,
        "\ngraphData:\n", graphData,
        "\ngetGraphData() options:", options,
      );

      resetInstance("Rendering...");
      await deps.renderGraph(deps.state.graph, graphData);
      await deps.setupGraph(deps.state.graph, graphJSON);
      resetInstance();
      deps.updateOptionsHint(graphData.combos.length);
    }).catch(error => {
      console.error(error);
      resetGraphLoaderState(deps.state);
      deps.clearExplorer();
      deps.updateOptionsHint(0);
      const showError = () => resetSidebar(String(error));
      void (deps.state.graph?.clear().then(showError, e => { console.error(e); showError(); }) ?? showError());
    });

    return true;
  };

  readFile() || resetInstance("Please load the graph.json file.");
  fileInput.addEventListener("change", readFile);
};
