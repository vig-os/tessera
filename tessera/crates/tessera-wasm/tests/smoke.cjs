// Node smoke test for the wasm-bindgen bindings (proves the generated wasm + JS glue LOAD and EXECUTE
// in a JS runtime — the binding mechanism; the verification *logic* is covered by the Rust unit tests).
// Run: node smoke.cjs <path-to-generated tessera_wasm.js> (the `wasm-bindgen --target nodejs` output).
const assert = require("assert");
const t = require(process.argv[2]);

// the exported functions are callable from JS and return the right types.
assert.strictEqual(typeof t.version(), "string");
assert.ok(t.version().length > 0, "version() should be non-empty");

// malformed input never throws — it returns false (the wasm boundary is panic-safe for verifiers).
assert.strictEqual(t.verify_manifest("not a manifest"), false);
assert.strictEqual(t.verify_signature("a", "b", "c"), false);
assert.strictEqual(t.product_id("not json"), "");

console.log("tessera-wasm node smoke OK — version=" + t.version());
