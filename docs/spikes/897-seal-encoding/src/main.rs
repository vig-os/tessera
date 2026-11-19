//! Spike #897 bake-off runner. See README.md for the commands and docs/spikes/897-seal-encoding.md
//! for the results.

mod attacks;
mod cbor;
mod options;

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tessera_core::hash::digest;

fn repo_root() -> PathBuf {
    let out = Command::new("git").args(["rev-parse", "--show-toplevel"]).output().expect("git");
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

/// The tracked `.tsra` files, in `git ls-files` order.
fn tsra_files() -> Vec<PathBuf> {
    let root = repo_root();
    let out = Command::new("git").args(["ls-files", "*.tsra"]).current_dir(&root).output().expect("git");
    String::from_utf8(out.stdout).unwrap().lines().map(|l| root.join(l)).collect()
}

fn member(path: &Path, name: &str) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut buf = Vec::new();
    z.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

fn label(path: &Path) -> String {
    let root = repo_root();
    path.strip_prefix(&root).unwrap().display().to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("corpus") => corpus(),
        Some("attacks") => attacks_cmd(),
        Some("vectors") => vectors(Path::new(args.get(2).map(String::as_str).unwrap_or("out/vectors"))),
        Some("products") => products(Path::new(args.get(2).map(String::as_str).unwrap_or("out/products"))),
        Some("libprobe") => libprobe(),
        Some("numbers") => numbers(),
        Some("floats") => floats(Path::new(args.get(2).map(String::as_str).unwrap_or("out/vectors/floats.tsv"))),
        _ => {
            eprintln!("usage: seal-encoding-spike corpus|attacks|vectors [dir]|products [dir]|libprobe|numbers|floats [file]");
            std::process::exit(2);
        }
    }
}

// ------------------------------------------------------------------------------------ corpus

/// A′ and B over the 13 tracked products: does each option verify its own encoding, does A′ keep the
/// golden seal, and how big is each stored form.
fn corpus() {
    let goldens: Value =
        serde_json::from_slice(&fs::read(repo_root().join("tessera/corpus/corpus.json")).unwrap()).unwrap();
    let golden: BTreeMap<String, String> = goldens
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["name"].as_str().unwrap().to_owned(), g["manifest_hash"].as_str().unwrap().to_owned()))
        .collect();
    println!("| product | A verify | A′ seal == A seal | golden | A′ verify | B verify | B round-trip | stored JSON | JCS | CBOR | CBOR, byte digests |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let (mut n, mut same, mut apv, mut bv, mut rt, mut id_ok) = (0, 0, 0, 0, 0, 0);
    let (mut s_json, mut s_jcs, mut s_cbor, mut s_cborb) = (0, 0, 0, 0);
    for f in tsra_files() {
        n += 1;
        let text = String::from_utf8(member(&f, "manifest.json")).unwrap();
        let a = options::verify_a(&text);
        let base = attacks::Base::new(text.clone());
        let ap_seal = digest(&base.ap);
        let ap = options::verify_aprime(&base.ap, Some(&ap_seal));
        let b = options::verify_b(&base.b, Some(&base.b_seal));
        let decoded = cbor::decode_strict(&base.b).unwrap();
        // decode → encode gives the bytes back; decode → JSON is the same value under JCS (numeric
        // reduction turns `1.0` into `1`, which JCS writes identically)
        let round = cbor::encode(&decoded).unwrap() == base.b
            && serde_jcs::to_vec(&cbor::to_serde_json(&decoded).unwrap()).unwrap() == serde_jcs::to_vec(&base.bare).unwrap();
        let bb = options::b_bytes(&base.obj, true);
        // `id` = digest(JCS(id_inputs)); JCS nests, so those bytes sit verbatim inside the A′ bytes.
        let idj = serde_jcs::to_vec(&base.obj["id_inputs"]).unwrap();
        let id_slice = digest(&idj) == base.obj["id"].as_str().unwrap()
            && base.ap.windows(idj.len()).any(|w| w == idj.as_slice());
        id_ok += usize::from(id_slice);
        let stem = f.file_stem().unwrap().to_str().unwrap();
        let g = match golden.get(stem) {
            Some(h) if f.starts_with(repo_root().join("tessera/corpus")) => {
                if *h == ap_seal { "= golden" } else { "≠ golden" }
            }
            _ => "(no golden)",
        };
        same += usize::from(ap_seal == base.seal);
        apv += usize::from(matches!(ap, options::Verdict::Verified(_)));
        bv += usize::from(matches!(b, options::Verdict::Verified(_)));
        rt += usize::from(round);
        s_json += text.len();
        s_jcs += base.ap.len();
        s_cbor += base.b.len();
        s_cborb += bb.len();
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            label(&f),
            a.short(),
            ap_seal == base.seal,
            g,
            ap.short(),
            b.short(),
            round,
            text.len(),
            base.ap.len(),
            base.b.len(),
            bb.len()
        );
    }
    println!(
        "| **total {n}** | | **{same}/{n}** | | **{apv}/{n}** | **{bv}/{n}** | **{rt}/{n}** | {s_json} | {s_jcs} ({:.0}%) | {s_cbor} ({:.0}%) | {s_cborb} ({:.0}%) |",
        100.0 * s_jcs as f64 / s_json as f64,
        100.0 * s_cbor as f64 / s_json as f64,
        100.0 * s_cborb as f64 / s_json as f64
    );
    println!("\n`id` = digest of the `id_inputs` byte slice found verbatim inside the A′ bytes: {id_ok}/{n}");
}

// ------------------------------------------------------------------------------------ attacks

fn attacks_cmd() {
    let bases: Vec<(String, attacks::Base)> = tsra_files()
        .iter()
        .map(|f| (label(f), attacks::Base::new(String::from_utf8(member(f, "manifest.json")).unwrap())))
        .collect();
    for (l, b) in &bases {
        assert!(attacks::a_reads(&b.text), "{l}: the unmodified product must verify under A");
    }
    println!("## Summary over the {} tracked products (count × outcome)\n", bases.len());
    println!("| id | attack | A (alpha.6) | A′ | B |");
    println!("|---|---|---|---|---|");
    let mut detail = String::new();
    for at in attacks::all() {
        let mut tally: [BTreeMap<String, usize>; 3] = Default::default();
        for (l, base) in &bases {
            let Some(c) = (at.build)(base) else { continue };
            let outs = [
                c.a.as_deref().map(attacks::run_a),
                c.ap.as_ref().map(|(b, s)| attacks::run_ap(b, s.as_deref())),
                c.b.as_ref().map(|(b, s)| attacks::run_b(b, s.as_deref())),
            ];
            for (i, o) in outs.iter().enumerate() {
                let cls = o.as_deref().map(attacks::class).unwrap_or_else(|| "impossible (no such form)".into());
                *tally[i].entry(cls).or_default() += 1;
            }
            if l.ends_with("recon_int16.tsra") {
                detail.push_str(&format!("### {} — {}\n\n", at.id, at.what));
                for (name, o) in ["A", "A′", "B"].iter().zip(&outs) {
                    detail.push_str(&format!(
                        "- {name}: {}\n",
                        o.as_deref().unwrap_or("impossible: the encoding has no such form")
                    ));
                }
                detail.push('\n');
            }
        }
        let cell = |t: &BTreeMap<String, usize>| {
            t.iter().map(|(k, v)| format!("{v}× {k}")).collect::<Vec<_>>().join("<br>")
        };
        println!("| {} | {} | {} | {} | {} |", at.id, at.what, cell(&tally[0]), cell(&tally[1]), cell(&tally[2]));
    }
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for (_, base) in &bases {
        if let Some(o) = attacks::s7_b0(base) {
            *tally.entry(attacks::class(&o)).or_default() += 1;
        }
    }
    println!(
        "\nS7 under profile B0 (no numeric reduction): {}",
        tally.iter().map(|(k, v)| format!("{v}× {k}")).collect::<Vec<_>>().join(", ")
    );
    println!("\n## Verbatim verdicts for tessera/corpus/files/recon_int16.tsra\n");
    println!("{}", detail.trim_end());
}

// ------------------------------------------------------------------------------------ vectors

const U64_MAX: u64 = u64::MAX;
const I64_MIN: i64 = i64::MIN;
const EPOCH_NS: u64 = 1_730_000_000_123_456_789;
const P53_1: u64 = 9_007_199_254_740_993;

/// Byte vectors for the Python and JavaScript readers (`py/agree.py`, `js/agree.mjs`).
fn vectors(dir: &Path) {
    for d in ["aprime", "b", "b0", "bad"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    let w = |p: &str, b: &[u8]| fs::write(dir.join(p), b).unwrap();

    // 64-bit values in each option's spelling.
    w(
        "ints.json",
        format!("{{\"epoch_ns\":{EPOCH_NS},\"i64_min\":{I64_MIN},\"p53_plus_1\":{P53_1},\"u64_max\":{U64_MAX}}}").as_bytes(),
    );
    w(
        "ints_str.json",
        format!("{{\"epoch_ns\":\"{EPOCH_NS}\",\"i64_min\":\"{I64_MIN}\",\"p53_plus_1\":\"{P53_1}\",\"u64_max\":\"{U64_MAX}\"}}")
            .as_bytes(),
    );
    let ints = cbor::Value::Map(vec![
        ("epoch_ns".into(), cbor::Value::Uint(EPOCH_NS)),
        ("i64_min".into(), cbor::Value::Nint((-1i128 - I64_MIN as i128) as u64)),
        ("p53_plus_1".into(), cbor::Value::Uint(P53_1)),
        ("u64_max".into(), cbor::Value::Uint(U64_MAX)),
        ("small".into(), cbor::Value::Uint(42)),
    ]);
    w("ints.cbor", &cbor::encode(&ints).unwrap());

    // The 13 products in A′ and B form, with their seals.
    let mut seals = serde_json::Map::new();
    for f in tsra_files() {
        let base = attacks::Base::new(String::from_utf8(member(&f, "manifest.json")).unwrap());
        let stem = label(&f).replace('/', "__").replace(".tsra", "");
        w(&format!("aprime/{stem}.json"), &base.ap);
        w(&format!("b/{stem}.cbor"), &base.b);
        w(&format!("b0/{stem}.cbor"), &options::b_bytes_as(&base.obj, false, cbor::Profile::B0));
        seals.insert(stem, json!({"aprime": digest(&base.ap), "b": base.b_seal}));
    }
    w("seals.json", serde_json::to_string_pretty(&seals).unwrap().as_bytes());

    // Non-deterministic or out-of-profile CBOR: does a generic decoder notice?
    let bad: &[(&str, &[u8])] = &[
        ("int5_non_shortest", &[0x18, 0x05]),
        ("int64_non_shortest", &[0x19, 0x00, 0x40]),
        ("text_indefinite", &[0x7f, 0x61, 0x61, 0xff]),
        ("map_unsorted", &[0xa2, 0x61, 0x62, 0x01, 0x61, 0x61, 0x02]),
        ("map_duplicate_key", &[0xa2, 0x61, 0x61, 0x01, 0x61, 0x61, 0x02]),
        ("float_1_5_as_f32", &[0xfa, 0x3f, 0xc0, 0x00, 0x00]),
        ("float_nan", &[0xf9, 0x7e, 0x00]),
        ("bignum_tag2", &[0xc2, 0x41, 0x01]),
        ("undefined", &[0xf7]),
        ("text_lone_surrogate_cesu8", &[0x63, 0xed, 0xa0, 0x80]),
        ("trailing_byte", &[0x01, 0x01]),
    ];
    for (name, b) in bad {
        let verdict = match cbor::decode_strict(b) {
            Ok(_) => "accepted".to_owned(),
            Err(e) => format!("refused: {e}"),
        };
        w(&format!("bad/{name}.cbor"), b);
        println!("rust strict decoder  {name:28} {verdict}");
    }
    w("lone_surrogate.json", br#"{"description":"\ud800"}"#);
    w("shape_float.json", br#"{"shape":[64.0,64,64]}"#);
    let shape_float = cbor::Value::Map(vec![(
        "shape".into(),
        cbor::Value::Array(vec![cbor::Value::Float(64.0), cbor::Value::Uint(64), cbor::Value::Uint(64)]),
    )]);
    w("shape_float.cbor", &cbor::encode_as(&shape_float, cbor::Profile::B0).unwrap()); // B0: B can't hold it

    // What the Rust A reader and the B decoder make of the 64-bit vectors.
    let text = fs::read_to_string(dir.join("ints.json")).unwrap();
    println!(
        "rust A parse_strict  ints.json                    {}",
        match tessera_core::canonical::parse_strict(&text) {
            Ok(_) => "accepted".into(),
            Err(e) => format!("refused: {e}"),
        }
    );
    println!(
        "rust B decode_strict ints.cbor                    {}",
        cbor::to_json_text(&cbor::decode_strict(&fs::read(dir.join("ints.cbor")).unwrap()).unwrap())
    );
    println!("wrote vectors to {}", dir.display());
}

// ------------------------------------------------------------------------------------ products

/// Two demo products for the legibility test: the same recon product with one 64-bit field, as A′
/// and as B. Members are STORED with the zip-epoch time; the exact §6 byte layout is not reproduced.
fn products(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let src = repo_root().join("tessera/corpus/files/recon_int16.tsra");
    let text = String::from_utf8(member(&src, "manifest.json")).unwrap();
    let block = member(&src, "blocks/volume");
    let mut obj = tessera_core::canonical::parse_strict(&text).unwrap();
    obj.as_object_mut().unwrap().remove("manifest_hash");
    obj["description"] = json!("int16 CT volume, with the acquisition start time");
    let field = json!({
        "id": "daq_start_ns", "dtype": "int64", "required": false, "recommended": false,
        "sensitivity": "public",
        "description": "DAQ start, nanoseconds since the Unix epoch (UTC)",
    });
    obj["schema"]["fields"].as_array_mut().unwrap().push(field);

    // A′: 64-bit value as a decimal string (#895 style).
    let mut ap_obj = obj.clone();
    ap_obj["metadata"]["daq_start_ns"] = json!(EPOCH_NS.to_string());
    let ap = tessera_core::canonical::to_bytes(&ap_obj).unwrap();
    assert!(matches!(options::verify_aprime(&ap, Some(&digest(&ap))), options::Verdict::Verified(_)));

    // B: 64-bit value as a native CBOR unsigned integer.
    let mut b_obj = cbor::from_json(&obj, false);
    if let cbor::Value::Map(es) = &mut b_obj {
        for (k, v) in es.iter_mut() {
            if k == "metadata" {
                if let cbor::Value::Map(md) = v {
                    md.push(("daq_start_ns".into(), cbor::Value::Uint(EPOCH_NS)));
                }
            }
        }
    }
    let b = cbor::encode(&b_obj).unwrap();
    assert!(matches!(options::verify_b(&b, Some(&digest(&b))), options::Verdict::Verified(_)));

    let readme = "This .tsra stores its manifest as deterministic CBOR (RFC 8949 section 4.2.1) in manifest.cbor.\n\
        Decode it with any CBOR reader, for example:\n  \
        tessera inspect --json product.tsra\n  \
        python3 -m cbor2.tool manifest.cbor   (pip install cbor2)\n  \
        cbor-diag manifest.cbor\n\
        manifest.seal holds blake3 over the exact bytes of manifest.cbor.\n";
    write_tsra(
        &dir.join("aprime.tsra"),
        &[("manifest.json", ap.clone()), ("manifest.seal", digest(&ap).into_bytes()), ("blocks/volume", block.clone())],
    );
    write_tsra(
        &dir.join("b.tsra"),
        &[
            ("manifest.cbor", b.clone()),
            ("manifest.seal", digest(&b).into_bytes()),
            ("blocks/volume", block),
            ("aux/README", readme.as_bytes().to_vec()),
        ],
    );
    println!("aprime.tsra manifest.json {} bytes, seal {}", ap.len(), digest(&ap));
    println!("b.tsra      manifest.cbor {} bytes, seal {}", b.len(), digest(&b));
}

fn write_tsra(path: &Path, members: &[(&str, Vec<u8>)]) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true);
    z.start_file("mimetype", o).unwrap();
    z.write_all(b"application/vnd.tessera").unwrap();
    for (name, bytes) in members {
        z.start_file(*name, o).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
}

// ------------------------------------------------------------------------------------ libprobe

/// Do the maintained Rust CBOR libraries refuse non-deterministic input on their own?
fn libprobe() {
    fn mini<T: for<'b> minicbor::Decode<'b, ()> + std::fmt::Debug>(b: &[u8]) -> String {
        match minicbor::decode::<T>(b) {
            Ok(v) => format!("accepted → {v:?}"),
            Err(e) => format!("refused ({e})"),
        }
    }
    fn c4<T: for<'de> serde::Deserialize<'de> + std::fmt::Debug>(b: &[u8]) -> String {
        match cbor4ii::serde::from_slice::<T>(b) {
            Ok(v) => format!("accepted → {v:?}"),
            Err(e) => format!("refused ({e})"),
        }
    }
    type M = BTreeMap<String, u64>;
    let cases: Vec<(&str, Vec<u8>, String, String)> = vec![
        ("5 with a 1-byte argument", vec![0x18, 0x05], mini::<u64>(&[0x18, 0x05]), c4::<u64>(&[0x18, 0x05])),
        ("64 with a 2-byte argument", vec![0x19, 0, 0x40], mini::<u64>(&[0x19, 0, 0x40]), c4::<u64>(&[0x19, 0, 0x40])),
        (
            "indefinite text \"a\"",
            vec![0x7f, 0x61, 0x61, 0xff],
            mini::<String>(&[0x7f, 0x61, 0x61, 0xff]),
            c4::<String>(&[0x7f, 0x61, 0x61, 0xff]),
        ),
        (
            "map keys out of order {b:1,a:2}",
            vec![0xa2, 0x61, 0x62, 1, 0x61, 0x61, 2],
            mini::<M>(&[0xa2, 0x61, 0x62, 1, 0x61, 0x61, 2]),
            c4::<M>(&[0xa2, 0x61, 0x62, 1, 0x61, 0x61, 2]),
        ),
        (
            "duplicate key {a:1,a:2}",
            vec![0xa2, 0x61, 0x61, 1, 0x61, 0x61, 2],
            mini::<M>(&[0xa2, 0x61, 0x61, 1, 0x61, 0x61, 2]),
            c4::<M>(&[0xa2, 0x61, 0x61, 1, 0x61, 0x61, 2]),
        ),
        (
            "1.5 as f32 (not preferred)",
            vec![0xfa, 0x3f, 0xc0, 0, 0],
            mini::<f64>(&[0xfa, 0x3f, 0xc0, 0, 0]),
            c4::<f64>(&[0xfa, 0x3f, 0xc0, 0, 0]),
        ),
        ("bignum tag 2 as u64", vec![0xc2, 0x41, 0x01], mini::<u64>(&[0xc2, 0x41, 0x01]), c4::<u64>(&[0xc2, 0x41, 0x01])),
        (
            "CESU-8 lone surrogate",
            vec![0x63, 0xed, 0xa0, 0x80],
            mini::<String>(&[0x63, 0xed, 0xa0, 0x80]),
            c4::<String>(&[0x63, 0xed, 0xa0, 0x80]),
        ),
    ];
    println!("| input | bytes | minicbor 2.3.1 | cbor4ii 1.2.5 (serde) |");
    println!("|---|---|---|---|");
    for (what, b, m, c) in cases {
        println!("| {what} | `{}` | {m} | {c} |", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    }
    // And the encoders: do they emit the profile form for a float on their own?
    let mut mb = Vec::new();
    minicbor::encode(1.5f64, &mut mb).unwrap();
    let c4b = cbor4ii::serde::to_vec(Vec::new(), &1.5f64).unwrap();
    println!(
        "\nencode 1.5f64: minicbor `{}`, cbor4ii `{}`, profile `{}`",
        hex(&mb),
        hex(&c4b),
        hex(&cbor::encode(&cbor::Value::Float(1.5)).unwrap())
    );
    let mut mm = Vec::new();
    let m: BTreeMap<&str, u8> = [("bb", 1), ("c", 2)].into_iter().collect();
    minicbor::encode(&m, &mut mm).unwrap();
    println!(
        "encode {{\"bb\":1,\"c\":2}} (BTreeMap): minicbor `{}` (writes the map's iteration order; it does not sort), profile `{}` (RFC 8949 order puts the shorter key first)",
        hex(&mm),
        hex(&cbor::encode(&cbor::Value::Map(vec![("bb".into(), cbor::Value::Uint(1)), ("c".into(), cbor::Value::Uint(2))])).unwrap())
    );
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ------------------------------------------------------------------------------------ numbers

/// How the JCS encoder treats integers past 2^53: does serde_jcs print the integer, or its double?
fn numbers() {
    for v in [json!(9007199254740993u64), json!(EPOCH_NS), json!(u64::MAX), json!(i64::MIN), json!(1e21), json!(1e-7)] {
        println!("serde_jcs {v:>22} → {}", String::from_utf8(serde_jcs::to_vec(&v).unwrap()).unwrap());
    }
    let parsed: Value = serde_json::from_str("9007199254740993").unwrap();
    println!("serde_json parses 9007199254740993 as u64: {}", parsed.is_u64());
    let (hi, lo) = (json!({"seed": 9007199254740993u64}), json!({"seed": 9007199254740992u64}));
    println!(
        "A seal rule: digest(JCS({{seed: 2^53+1}})) == digest(JCS({{seed: 2^53}})): {}",
        digest(&serde_jcs::to_vec(&hi).unwrap()) == digest(&serde_jcs::to_vec(&lo).unwrap())
    );
    println!(
        "A′ seal rule: digest(b'{{\"seed\":9007199254740993}}') == digest(b'{{\"seed\":9007199254740992}}'): {}",
        digest(br#"{"seed":9007199254740993}"#) == digest(br#"{"seed":9007199254740992}"#)
    );
    for t in ["9007199254740992", "1730000000123456800", "1730000000123456789"] {
        let v: Value = serde_json::from_str(t).unwrap();
        println!("is {t} its own JCS form: {}", String::from_utf8(serde_jcs::to_vec(&v).unwrap()).unwrap() == t);
    }
}

// ------------------------------------------------------------------------------------ floats

/// Random doubles (uniform over bit patterns, so every exponent) plus edge cases, each with the JCS
/// text serde_jcs writes and the preferred-serialization CBOR bytes (profile B0: numeric reduction
/// is a separate rule no library tested implements), for `py/floats.py` and `js/floats.mjs` to
/// reproduce independently.
fn floats(path: &Path) {
    let mut vals: Vec<f64> = vec![
        0.0, -0.0, 1.0, -1.0, 0.1, 0.5, 1.5, 65504.0, 65505.0, 1e21, 1e-7, 1e-6, 1e20, 5e-324, 2.2250738585072014e-308,
        1.7976931348623157e308, 9007199254740993.0, 123456789012345680000.0, 0.30000000000000004, 3.4028234663852886e38,
        5.960464477539063e-8, 6.103515625e-5, 1e300, 1e-300,
    ];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    while vals.len() < 200_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let f = f64::from_bits(x);
        if f.is_finite() {
            vals.push(f);
        }
        // and as many short-mantissa values, which narrow to f32/f16 under CBOR
        let g = (x >> 40) as f32 / 1024.0 * if x & 1 == 0 { 1.0 } else { -1.0 };
        vals.push(g as f64);
    }
    let mut out = String::new();
    for f in &vals {
        let jcs = String::from_utf8(serde_jcs::to_vec(f).unwrap()).unwrap();
        out.push_str(&format!("{:016x}\t{jcs}\t{}\n", f.to_bits(), hex(&cbor::encode_as(&cbor::Value::Float(*f), cbor::Profile::B0).unwrap())));
    }
    fs::write(path, out).unwrap();
    println!("wrote {} doubles to {}", vals.len(), path.display());
}
