import { Shape as Base } from "./shape.ts";
import { Shape as Circle } from "./circle.ts";

self.postMessage(
  [new Base().constructor.name, new Circle().constructor.name].join(" "),
);
