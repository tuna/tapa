/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { resetInstance, resetSidebar, updateExplorer } from "../sidebar.js";
import { getGraphData } from "../parser.js";

/** @typedef {{
 *   getOptions: () => GetGraphDataOptions,
 *   renderGraph: (graph: import("@antv/g6").Graph, graphData: GraphData) => Promise<void>,
 *   setupGraph: (graph: import("@antv/g6").Graph, graphJSON: GraphJSON) => Promise<void>,
 *   setFilename: (filename: string) => void,
 *   setGraphJSON: (graphJSON: GraphJSON | undefined) => void,
 *   setGraphData: (graphData: GraphData) => void,
 *   setOptions: (options: GetGraphDataOptions) => void,
 *   updateOptionsHint: (comboCount: number) => void,
 * }} FileLoaderDeps
 */

/** @param {import("@antv/g6").Graph} graph
 * @param {HTMLInputElement & { files: FileList }} fileInput
 * @param {FileLoaderDeps} deps */
export const setupFileInput = (graph, fileInput, deps) => {
  const readFile = () => {
    /** @type {File | undefined} */
    const file = fileInput.files[0];

    if (!file) return false;
    if (file.type !== "application/json") {
      console.warn("File type is not application/json!");
    }

    deps.setFilename(file.name);

    resetSidebar("Loading...");
    file.text().then(async text => {
      /** @satisfies {GraphJSON} */
      const graphJSON = JSON.parse(text);
      if (!graphJSON?.tasks) {
        deps.setGraphJSON(undefined);
        resetInstance("Invalid graph.json, please retry.");
        return;
      }

      updateExplorer(graphJSON);

      const options = deps.getOptions();
      const graphData = getGraphData(graphJSON, options);

      deps.setGraphJSON(graphJSON);
      deps.setGraphData(graphData);
      deps.setOptions(options);
      Object.assign(globalThis, { graphJSON, graphData });

      console.debug(
        `${file.name}:\n`, graphJSON,
        "\ngraphData:\n", graphData,
        "\ngetGraphData() options:", options,
      );

      resetInstance("Rendering...");
      await deps.renderGraph(graph, graphData);
      await deps.setupGraph(graph, graphJSON);
      resetInstance();
      deps.updateOptionsHint(graphData.combos.length);
    }).catch(error => {
      console.error(error);
      deps.setGraphJSON(undefined);
      resetInstance(String(error));
    });

    return true;
  };

  readFile() || resetInstance("Please load the graph.json file.");
  fileInput.addEventListener("change", readFile);
};
