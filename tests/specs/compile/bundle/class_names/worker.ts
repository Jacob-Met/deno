import { Shape as Base } from "./shape.ts";
import { Shape as Circle } from "./circle.ts";

self.postMessage([Base.name, Circle.name].join(" "));
