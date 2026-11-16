const encoder = new TextEncoder();
const decoder = new TextDecoder();
let handled = 0;
const server = Deno.serve(
  { hostname: "127.0.0.1", port: 0, onListen() {} },
  (_request) => {
    handled++;
    return new Response("accepted");
  },
);

async function responseStatus(raw: string): Promise<string> {
  const conn = await Deno.connect({
    hostname: "127.0.0.1",
    port: server.addr.port,
  });
  try {
    await conn.write(encoder.encode(raw));
    const chunk = new Uint8Array(256);
    let head = "";
    while (!head.includes("\r\n")) {
      const length = await conn.read(chunk);
      if (length === null) {
        throw new Error("server closed without a complete status line");
      }
      head += decoder.decode(chunk.subarray(0, length));
      if (head.length > 4096) {
        throw new Error("response status line too long");
      }
    }
    return head.slice(0, head.indexOf("\r\n"));
  } finally {
    conn.close();
  }
}

try {
  const cases = [
    ["GET / HTTP/1.1\r\nConnection: close\r\n\r\n", "HTTP/1.1 400"],
    ["HEAD / HTTP/1.1\r\nConnection: close\r\n\r\n", "HTTP/1.1 400"],
    [
      "POST / HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
      "HTTP/1.1 400",
    ],
    [
      "GET http://example.com/ HTTP/1.1\r\nConnection: close\r\n\r\n",
      "HTTP/1.1 400",
    ],
    ["GET / HTTP/1.0\r\nConnection: close\r\n\r\n", "HTTP/1.0 200"],
    [
      "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
      "HTTP/1.1 200",
    ],
  ] as const;
  for (const [request, expected] of cases) {
    const actual = await responseStatus(request);
    if (!actual.startsWith(expected)) {
      throw new Error(`expected ${expected}, got ${actual}`);
    }
  }
  if (handled !== 2) {
    throw new Error(`expected 2 requests dispatched, got ${handled}`);
  }
  console.log("HTTP/1.1 requires Host; HTTP/1.0 remains compatible");
} finally {
  await server.shutdown();
}
