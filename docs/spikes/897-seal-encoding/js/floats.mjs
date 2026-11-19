// Does JavaScript's JCS (canonicalize) produce the same text as serde_jcs for 200k doubles?
// Usage: node js/floats.mjs out/vectors/floats.tsv
import { readFileSync } from 'node:fs';
import canonicalize from 'canonicalize';

let n = 0, ok = 0, shown = 0;
for (const line of readFileSync(process.argv[2] ?? 'out/vectors/floats.tsv', 'utf8').split('\n')) {
  if (!line) continue;
  const [bits, rustJcs] = line.split('\t');
  const f = Buffer.from(bits, 'hex').readDoubleBE(0);
  n++;
  const js = canonicalize(f);
  if (js === rustJcs) ok++;
  else if (shown++ < 5) console.log(`  differs: ${f}: rust ${rustJcs} js ${js}`);
}
console.log(`JCS  (serde_jcs vs JS canonicalize 5.1.0):              ${ok}/${n} identical`);
