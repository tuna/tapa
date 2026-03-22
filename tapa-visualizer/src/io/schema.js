/*
 * Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
 * All rights reserved. The contributor(s) of this file has/have agreed to the
 * RapidStream Contributor License Agreement.
 */

"use strict";

import { z } from "zod";

const subTaskArgSchema = z.strictObject({
  arg: z.string(),
  cat: z.string(),
});

const subTaskSchema = z.strictObject({
  args: z.record(z.string(), subTaskArgSchema),
  step: z.number(),
});

const fifoEndpointSchema = z.tuple([z.string(), z.number()]);

const fifoSchema = z.strictObject({
  produced_by: fifoEndpointSchema.optional(),
  consumed_by: fifoEndpointSchema.optional(),
  depth: z.number().optional(),
});

const portSchema = z.strictObject({
  name: z.string(),
  cat: z.string(),
  type: z.string(),
  width: z.number(),
});

const upperTaskSchema = z.strictObject({
  level: z.literal("upper"),
  target: z.string(),
  vendor: z.string(),
  tasks: z.record(z.string(), z.array(subTaskSchema)),
  fifos: z.record(z.string(), fifoSchema),
  ports: z.array(portSchema),
  code: z.string(),
});

const lowerTaskSchema = z.strictObject({
  level: z.literal("lower"),
  target: z.string(),
  vendor: z.string(),
  ports: z.array(portSchema).optional(),
  code: z.string(),
});

export const graphJsonSchema = z.strictObject({
  top: z.string(),
  tasks: z.record(z.string(), z.union([upperTaskSchema, lowerTaskSchema])),
  cflags: z.array(z.string()),
});

/** @param {string} text
 * @returns {GraphJSON} */
export const parseGraphJSON = text => {
  /** @type {unknown} */
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new TypeError("Invalid graph.json: file is not valid JSON.");
  }

  const result = graphJsonSchema.safeParse(/** @type {unknown} */ (parsed));
  if (!result.success) {
    const issue = result.error.issues[0];
    const where = issue?.path.join(".") || "<root>";
    const message = issue?.message || "unknown validation error";
    throw new TypeError(`Invalid graph.json: ${where}: ${message}`);
  }
  return result.data;
};
