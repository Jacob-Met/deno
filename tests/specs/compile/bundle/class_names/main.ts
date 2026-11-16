import { Shape as Base } from "./shape.ts";
import { Shape as Circle } from "./circle.ts";

console.log(new Base().constructor.name, new Circle().constructor.name);
class ExplicitName {
  static name = "unchanged";
}
console.log("explicit", ExplicitName.name);

const worker = new Worker(new URL("./worker.ts", import.meta.url), {
  type: "module",
});
const done = Promise.withResolvers<void>();
worker.onmessage = (event) => {
  console.log("worker", event.data);
  worker.terminate();
  done.resolve();
};
worker.onerror = (event) => done.reject(event);
await done.promise;
