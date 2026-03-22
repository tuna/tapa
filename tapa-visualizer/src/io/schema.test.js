import { describe, expect, it } from "vitest";

import { parseGraphJSON } from "./schema.js";

describe("graph schema", () => {
  it("rejects malformed json", () => {
    expect(() => parseGraphJSON("{")).toThrow(
      "Invalid graph.json: file is not valid JSON.",
    );
  });

  it("rejects schema-invalid json", () => {
    expect(() => parseGraphJSON(JSON.stringify({
      top: "top",
      cflags: [],
      tasks: {
        top: {
          level: "upper",
          target: "hw",
          vendor: "xilinx",
          tasks: {},
          fifos: {},
          ports: [],
        },
      },
    }))).toThrow("Invalid graph.json: tasks.top.code");
  });
});
