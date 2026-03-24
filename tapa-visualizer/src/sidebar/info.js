/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { $, $text, append, getComboName } from "../helper.js";
import { parseArgs, parseFifos, parsePorts, showCode, ul } from "./dom.js";

/** @type {(node: NodeData) => HTMLElement} */
export const getNodeInfo = node => {
  const dl = append(
    $("dl"),
    $text("dt", "Node Name"), $text("dd", node.id),
    $text("dt", "Upper Task"), $text("dd", getComboName(node.combo ?? "<none>")),
  );

  /** @param {HTMLDListElement} target
   * @param {SubTask} subTask
   * @param {number} [i] */
  const appendSubTask = (target, { args, step }, i) => {
    const argsArr = Object.entries(args);
    if (typeof i === "number") {
      target.append($("dt", { textContent: `Sub-Task ${i}`, style: "padding: .25rem 0 .1rem; border-top: 1px solid var(--border);" }));
    }
    target.append(
      $text("dt", "Arguments"),
      argsArr.length > 0 ? parseArgs(argsArr) : $text("dd", "<none>"),
      $text("dt", "Step"),
      $text("dd", step),
    );
  };

  const { data } = node;
  if ("subTask" in data) appendSubTask(dl, data.subTask);
  else if ("subTasks" in data && Array.isArray(data.subTasks)) data.subTasks.forEach((subTask, i) => appendSubTask(dl, subTask, i));
  else console.warn("Selected node is missing data!", node);

  return dl;
};

/** @type {(task: Task, id: string) => HTMLElement[]} */
export const getTaskInfo = (task, id) => {
  const taskName = id.includes("/") ? id.slice(0, id.indexOf("/")) : id;

  const compactInfo = append(
    $("dl", { className: "compact" }),
    $text("dt", "Task Name:"), $text("dd", taskName),
    $text("dt", "Task Level:"), $text("dd", task.level),
    $text("dt", "Build Target:"), $text("dd", task.target),
    $text("dt", "Vendor:"), $text("dd", task.vendor),
    $text("dt", "Code:"), append($("dd"), showCode(task.code, taskName)),
  );

  const listInfo = append(
    $("dl"),
    $text("dt", "Ports"),
    task.ports && task.ports.length !== 0 ? append($("dd"), parsePorts(task.ports)) : $text("dd", "none"),
  );

  const elements = [compactInfo, listInfo];
  if (task.level === "upper") {
    const fifos = Object.entries(task.fifos);
    /** @type {HTMLLIElement[]} */
    const tasks = [];
    for (const name in task.tasks) {
      task.tasks[name].forEach((_, i) => tasks.push($("li", { textContent: `${name}/${i}` })));
    }
    listInfo.append(
      $text("dt", "FIFO Streams"),
      fifos.length !== 0 ? append($("dd"), parseFifos(fifos, taskName)) : $text("dd", "none"),
      $text("dt", "Sub-Tasks"),
      tasks.length !== 0 ? append($("dd"), ul(tasks)) : $text("dd", "none"),
    );
  }

  return elements;
};

/** @type {(combo: ComboData, graph: Graph) => HTMLElement} */
export const getComboInfo = (combo, graph) => append(
  $("dl"),
  $text("dt", "Combo Name"), $text("dd", combo.id),
  $text("dt", "Children"),
  append($("dd"), ul(graph.getChildrenData(combo.id).map(child => $text("li", child.id)))),
);
