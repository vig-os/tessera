// The compiled tessera-wasm over every corpus product, compared with the goldens.
// node wasm_products.cjs <tessera_wasm.js> <members.json> <corpus.json> <ingest-corpus.json>
// members.json comes from extract_members.py. A product passes when seal_verdict is "ok" (the seal,
// id and content_hash hold) and product_id and the seal member equal its golden id and manifest_hash.
const path = require("path");
const t = require(process.argv[2]);
const ps = require(process.argv[3]);
const goldens = new Map();
for (const g of require(process.argv[4])) goldens.set(g.name, g);
for (const g of require(process.argv[5]).fixtures) goldens.set(g.name, g);
const hex = (h) => Uint8Array.from(Buffer.from(h, "hex"));
let ok = 0;
for (const p of ps) {
  const name = path.basename(p.path, ".tsra");
  const m = hex(p.manifest_hex);
  const s = p.seal_hex === null ? undefined : hex(p.seal_hex);
  const verdict = t.seal_verdict(m, s);
  const id = t.product_id(m);
  const seal = s ? Buffer.from(s).toString() : null;
  const g = goldens.get(name);
  const pass = g !== undefined && verdict === "ok" && id === g.id && seal === g.manifest_hash;
  ok += pass;
  console.log(JSON.stringify({ name, verdict, id, seal, pass }));
}
console.log(`${ok}/${ps.length} products agreed (wasm)`);
process.exit(ok === ps.length ? 0 : 1);
