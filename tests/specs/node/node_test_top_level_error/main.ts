import { test } from "node:test";

test("registered", () => {});

throw new Error("uncaught after node:test registration");
