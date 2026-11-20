// Node smoke test for the wasm-bindgen bindings: proves the generated wasm + JS glue LOAD and EXECUTE
// in a JS runtime, and that the compiled wasm reaches every verdict of corpus/seal-cases.json, the
// file the native reader and the Python reference reader are held to (ADR-0066: all three agree).
// Run: node smoke.cjs <path-to-generated tessera_wasm.js> <path-to-seal-cases.json>
// (the `wasm-bindgen --target nodejs` output).
const assert = require("assert");
const fs = require("fs");
const t = require(process.argv[2]);

// the exported functions are callable from JS and return the right types.
assert.strictEqual(typeof t.version(), "string");
assert.ok(t.version().length > 0, "version() should be non-empty");

// malformed input never throws — it returns false (the wasm boundary is panic-safe for verifiers).
assert.strictEqual(t.verify_manifest("not a manifest", undefined), false);
assert.strictEqual(t.verify_signature("a", undefined, "b", "c"), false);
assert.strictEqual(t.product_id("not json"), "");

// every seal case, through the compiled wasm. A missing seal member is `null` in the file and
// `undefined` at the binding.
const cases = JSON.parse(fs.readFileSync(process.argv[3], "utf8")).cases;
assert.ok(cases.length > 0, "seal-cases.json has cases");
const disagreements = cases.filter(
  (c) => t.seal_verdict(c.manifest, c.seal === null ? undefined : c.seal) !== c.verdict,
);
assert.deepStrictEqual(
  disagreements.map((c) => c.name),
  [],
  "seal cases the wasm verdict disagrees with",
);
console.log(`${cases.length}/${cases.length} seal cases agreed (wasm)`);

console.log("tessera-wasm node smoke OK — version=" + t.version());
