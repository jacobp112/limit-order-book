// Loads the built lob.wasm, replays the example session through the same
// wrapper the page uses, and checks the digests printed by `lob replay`.
//
//   node explainer/check.mjs [path/to/lob.wasm]

import { readFile } from "node:fs/promises";
import { loadEngine } from "./lob.mjs";

const wasmPath = process.argv[2] ?? new URL("./lob.wasm", import.meta.url);
const journal = await readFile(new URL("../examples/journals/session.journal", import.meta.url), "utf8");
const engine = await loadEngine(await readFile(wasmPath));

let applied = 0;
for (const line of journal.split("\n")) {
  const command = line.split("#")[0].trim();
  if (!command) continue;
  const out = engine.apply(command);
  if (!out.ok) throw new Error(`${command}: ${out.error}`);
  applied++;
}

const hex = (v) => "0x" + BigInt.asUintN(64, v).toString(16).padStart(16, "0");
const expected = { state: "0x99d887fd8c46ef4e", events: "0x3992e01f91960e21" };
const actual = { state: hex(engine.stateDigest()), events: hex(engine.eventDigest()) };

const bad = engine.apply("submit sideways limit 1 1");
if (bad.ok || !bad.error.includes("sideways")) throw new Error("parse error not reported");
if (hex(engine.stateDigest()) !== expected.state) throw new Error("rejected line changed state");

console.log(`applied ${applied} commands; state ${actual.state}; events ${actual.events}`);
if (actual.state !== expected.state || actual.events !== expected.events) {
  console.error(`expected state ${expected.state}, events ${expected.events}`);
  process.exit(1);
}
console.log("ok: wasm engine matches `lob replay`");
