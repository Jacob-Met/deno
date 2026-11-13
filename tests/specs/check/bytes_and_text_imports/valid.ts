import dataBytes from "./data.txt" with { type: "bytes" };
import dataText from "./data.txt" with { type: "text" };

const bytes: Uint8Array<ArrayBuffer> = dataBytes;
const text: string = dataText;
console.log(bytes.length, text.length);
