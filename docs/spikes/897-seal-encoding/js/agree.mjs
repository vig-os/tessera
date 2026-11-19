// Spike #897 reader agreement, JavaScript side (node). Usage: node js/agree.mjs out/vectors
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { blake3 } from '@noble/hashes/blake3.js';
import { bytesToHex } from '@noble/hashes/utils.js';
import canonicalize from 'canonicalize';
import { decode, Encoder } from 'cbor-x';

const d = process.argv[2] ?? 'out/vectors';
const rd = (p) => readFileSync(join(d, p));
const digest = (b) => 'blake3:' + bytesToHex(blake3(b));
const WANT = {
  epoch_ns: 1730000000123456789n,
  i64_min: -(2n ** 63n),
  p53_plus_1: 9007199254740993n,
  u64_max: 2n ** 64n - 1n,
};
const exact = (got) =>
  Object.entries(WANT)
    .map(([k, v]) => {
      const g = got[k];
      const ok = typeof g === 'bigint' ? g === v : Number.isSafeInteger(g) && BigInt(g) === v;
      return `${k}=${ok ? 'exact' : `${String(g)} (${typeof g})`}`;
    })
    .join(', ');

console.log('## 64-bit values');
console.log('A  JSON.parse                  ', exact(JSON.parse(rd('ints.json'))));
const s = JSON.parse(rd('ints_str.json'));
console.log('A/A′ strings + BigInt()        ', exact(Object.fromEntries(Object.entries(s).map(([k, v]) => [k, BigInt(v)]))));
const c = decode(rd('ints.cbor'));
console.log('B  cbor-x decode               ', exact(c), '| types:', Object.entries(c).map(([k, v]) => `${k}:${typeof v}`).join(' '));
const c2 = new Encoder({ int64AsNumber: true, mapsAsObjects: true }).decode(rd('ints.cbor'));
console.log('B  cbor-x int64AsNumber        ', exact(c2));

console.log('\n## Corpus: does JavaScript reproduce the stored bytes and the seal?');
const seals = JSON.parse(rd('seals.json'));
const enc = new Encoder({ useRecords: false, mapsAsObjects: true, variableMapSize: true });
let okAp = 0, okB = 0, okB0 = 0, okSeal = 0, n = 0;
const firstDiff = [];
for (const [stem, sl] of Object.entries(seals)) {
  n++;
  const ap = rd(join('aprime', `${stem}.json`));
  const b = rd(join('b', `${stem}.cbor`));
  const sameAp = canonicalize(JSON.parse(ap.toString('utf8'))) === ap.toString('utf8');
  const re = Buffer.from(enc.encode(decode(b)));
  const sameB = re.equals(b);
  const b0 = rd(join('b0', `${stem}.cbor`));
  okB0 += Buffer.from(enc.encode(decode(b0))).equals(b0);
  const seal = digest(ap) === sl.aprime && digest(b) === sl.b;
  okAp += sameAp; okB += sameB; okSeal += seal;
  if (!sameB && firstDiff.length < 2) {
    const at = [...re].findIndex((x, i) => x !== b[i]);
    firstDiff.push(`${stem}: cbor-x re-encode ${re.length} B vs ${b.length} B, first difference at byte ${at}`);
  }
}
console.log(`A′ canonicalize(JSON.parse(b)) == b : ${okAp}/${n}`);
console.log(`B  cbor-x encode(decode(b)) == b    : ${okB}/${n}  (profile B, numeric reduction)`);
for (const l of firstDiff) console.log('   ', l);
console.log(`B0 cbor-x encode(decode(b)) == b    : ${okB0}/${n}  (profile B0, no numeric reduction)`);
// Why: JavaScript has one number type, so a float that holds an integer comes back as an integer.
const rs = Buffer.from(enc.encode({ rescale_slope: 1.0 })).toString('hex');
console.log(`    B0 cause: the float 1.0 (stored f9 3c00) decodes to the JS number 1 and re-encodes as ${rs.slice(-2)}, the integer 1`);
console.log(`blake3 seals over stored bytes match Rust : ${okSeal}/${n}`);
const hx = (v) => Buffer.from(enc.encode(v)).toString('hex');
console.log(`cbor-x writer: 1.5 → ${hx(1.5)} (profile f93e00); 100000.5 → ${hx(100000.5)} (profile fa47c35040)`);

console.log('\n## Out-of-profile CBOR: does cbor-x decode refuse it on its own?');
for (const f of readdirSync(join(d, 'bad')).sort()) {
  try {
    const v = decode(rd(join('bad', f)));
    const shown = typeof v === 'string' ? JSON.stringify(v) : typeof v === 'bigint' ? `${v}n` : JSON.stringify(v, (_, x) => (typeof x === 'bigint' ? `${x}n` : x));
    console.log(`  ${f.replace('.cbor', '').padEnd(28)} accepted → ${String(shown).slice(0, 40)}`);
  } catch (e) {
    console.log(`  ${f.replace('.cbor', '').padEnd(28)} refused: ${e.message}`);
  }
}

console.log('\n## Lone surrogate and a float-spelled integer');
const lone = JSON.parse(rd('lone_surrogate.json'));
console.log('A  JSON.parse lone surrogate    accepted →', JSON.stringify(lone.description), 'well-formed:', lone.description.isWellFormed());
console.log('A  JSON.parse shape            ', JSON.stringify(JSON.parse(rd('shape_float.json')).shape), '(64.0 and 64 are one JS number)');
console.log('B0 cbor-x decode shape         ', JSON.stringify(decode(rd('shape_float.cbor')).shape), '(the float/int distinction is lost)');
