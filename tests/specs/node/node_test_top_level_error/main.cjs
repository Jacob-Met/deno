const { test } = require("node:test");

test("registered", () => {});

throw new Error("uncaught after node:test registration");
