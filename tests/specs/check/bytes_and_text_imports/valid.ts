import bytes from "./data.txt" with { type: "bytes" };
import text from "./data.txt" with { type: "text" };
const { default: dynamicBytes } = await import("./hello.ts", {
  with: { type: "bytes" },
});
const { default: dynamicText } = await import("./hello.ts", {
  with: { type: "text" },
});
const byteBuffers: Uint8Array<ArrayBuffer>[] = [bytes, dynamicBytes];
const texts: string[] = [text, dynamicText];
console.log(byteBuffers.length, texts.length);
