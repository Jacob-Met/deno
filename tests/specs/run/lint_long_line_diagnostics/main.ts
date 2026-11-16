const decoder = new TextDecoder();

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const directory = await Deno.makeTempDir();

async function lint(file: string, flags: string[] = []) {
  const result = await new Deno.Command(Deno.execPath(), {
    args: ["lint", ...flags, file],
    cwd: directory,
    env: { NO_COLOR: "1", DENO_NO_UPDATE_CHECK: "1" },
    stdout: "piped",
    stderr: "piped",
  }).output();
  const stdout = decoder.decode(result.stdout);
  const stderr = decoder.decode(result.stderr);
  return { result, stdout, text: stdout + stderr };
}

try {
  const longSource = `const payload = "${
    "🧊".repeat(230)
  }"; console.log(payload); var broken = 2;\n`;
  const longFile = `${directory}/long.js`;
  await Deno.writeTextFile(longFile, longSource);

  const pretty = await lint(longFile);
  assert(!pretty.result.success, "expected the no-var diagnostic");
  const varColumn = longSource.indexOf("var broken") + 1;
  assert(pretty.text.includes(`:1:${varColumn}`), "original column lost");
  assert(pretty.text.includes("var broken"), "offending code not visible");
  assert(pretty.text.includes("^^^"), "error underline missing");
  assert(pretty.text.includes("…"), "long source not shortened");
  assert(
    pretty.text.match(/Long source line shortened/g)?.length === 1,
    "long-line guidance must appear once per file",
  );
  const snippets = pretty.text.split("\n").filter((line) =>
    line.includes("1 | ")
  );
  assert(
    snippets.every((line) => line.length < 260),
    "the source excerpt exceeded the window",
  );

  const laterFile = `${directory}/later.js`;
  await Deno.writeTextFile(laterFile, "const okay = 1;\n" + longSource);
  const later = await lint(laterFile);
  assert(!later.result.success, "later-line lint failure missing");
  assert(
    later.text.includes(`:2:${varColumn}`),
    "line number or original column lost",
  );
  assert(later.text.includes("2 | "), "snippet gutter lost its line");

  const compact = await lint(longFile, ["--compact"]);
  assert(!compact.result.success, "compact reporter lost lint failures");
  assert(compact.text.includes("no-var"), "compact diagnostic missing");
  assert(
    !compact.text.includes("Long source line shortened"),
    "compact reporter output changed",
  );

  const json = await lint(longFile, ["--json"]);
  assert(!json.result.success, "JSON reporter lost lint failures");
  const diagnostics = JSON.parse(json.stdout);
  assert(diagnostics.diagnostics.length >= 1, "JSON diagnostics missing");
  assert(
    !json.text.includes("Long source line shortened"),
    "JSON reporter output changed",
  );

  const shortFile = `${directory}/short.js`;
  await Deno.writeTextFile(shortFile, "var shortBad = 2;\n");
  const short = await lint(shortFile);
  assert(!short.result.success, "short lint failure missing");
  assert(short.text.includes("1 | var shortBad"), "normal snippet changed");
  assert(
    !short.text.includes("Long source line shortened"),
    "short source incorrectly shortened",
  );

  console.log("Long-line lint excerpts preserve locations and formats");
} finally {
  await Deno.remove(directory, { recursive: true });
}
