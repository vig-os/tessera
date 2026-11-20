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
const utf8 = (text) => new TextEncoder().encode(text);
assert.strictEqual(t.verify_manifest(utf8("not a manifest"), undefined), false);
assert.strictEqual(t.verify_signature(utf8("a"), undefined, "b", "c"), false);
assert.strictEqual(t.product_id(utf8("not json")), "");
assert.strictEqual(t.product_id(Uint8Array.of(0xff)), "");

// every seal case, through the compiled wasm, as the member BYTES (a Uint8Array, never a string
// decoded from them: the seal is over the stored bytes). A case holds them as the UTF-8 string they
// spell, or as `manifest_hex` when they are not UTF-8. A missing seal member is `null` in the file
// and `undefined` at the binding.
const cases = JSON.parse(fs.readFileSync(process.argv[3], "utf8")).cases;
assert.ok(cases.length > 0, "seal-cases.json has cases");
const bytes = (c) =>
  c.manifest_hex !== undefined ? Uint8Array.from(Buffer.from(c.manifest_hex, "hex")) : utf8(c.manifest);
const disagreements = cases.filter(
  (c) => t.seal_verdict(bytes(c), c.seal === null ? undefined : utf8(c.seal)) !== c.verdict,
);
assert.deepStrictEqual(
  disagreements.map((c) => c.name),
  [],
  "seal cases the wasm verdict disagrees with",
);
console.log(`${cases.length}/${cases.length} seal cases agreed (wasm)`);

console.log("tessera-wasm node smoke OK — version=" + t.version());
