import { describe } from "node:test";

describe("registered suite", () => {});

throw new Error("uncaught after node:test registration");
