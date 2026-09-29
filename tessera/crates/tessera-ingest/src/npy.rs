//! NumPy `.npy` → `array` (and `.npz` → a collection) — ADR-0056 §11's P1 array rows.
//!
//! # Why this is hand-parsed rather than `ndarray-npy`
//!
//! ADR-0056 §12 is explicit: *"`ndarray-npy` is deliberately **not** taken: NPY is a header parse plus
//! a memcpy, and the crate would drag `ndarray` into the tree for one format."* That is what this is —
//! a restricted-literal header parse ([`parse_header`]) and a buffer handed to
//! [`tessera_io::array::ArrayData::from_le_bytes`], which is the waist. No new dependency for `.npy`
//! at all.
//!
//! # The four things the format makes you decide
//!
//! An `.npy` header is self-describing about shape and dtype, so there is no inference anywhere — but
//! it is self-describing about three things Tessera's array primitive does not have, and one it
//! refuses:
//!
//! - **Byte order** (`<f8` vs `>f8`) — hazard **H6**. A naive `cast_slice` of a big-endian buffer is
//!   silently wrong on a little-endian host, so the buffer is byte-swapped to native before it reaches
//!   the waist. This is the **clean** lane, not a recorded transform: byte order carries no information
//!   a reader needs recovery instructions for, and recording it would make a `>f8` file and its `<f8`
//!   twin differ in the seal — which is exactly what ADR-0056 §5's `ingest_npy_endianness` fixture
//!   exists to forbid.
//! - **Fortran order** — a *reordering* of the buffer, so it is the **recorded** lane
//!   (`fortran_to_c_order`). Tessera arrays are C-order by definition (`ArraySpec.shape` +
//!   `tessera_io::array`'s chunk walk both assume it), and a reader comparing back to the original
//!   `.npy` needs to know the layout was rewritten.
//! - **A structured (record) dtype is a TABLE, not an array.** ADR-0056 §1 in its purest form: the
//!   shape of the data decides the primitive, and `dtype=[('x','<i4'),('y','<f8')]` is rows of typed
//!   fields. It is routed to the table lane rather than rejected or flattened into a meaningless
//!   numeric grid.
//! - **An object dtype (`|O`) is rejected, and is never unpickled.** A pickled `.npy` is arbitrary
//!   code, not data; `allow_pickle` is a remote-code-execution switch and Tessera does not have one.
//!   The error routes to `ingest blob`, which preserves the bytes without interpreting them.

use std::path::Path;

use tessera_core::block::array::ArraySpec;
use tessera_core::{Error, IngestTransform, Result};
use tessera_io::array::ArrayData;

use crate::canonical::{transform, CanonicalTable};

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: npy: {e}"))
}

/// `\x93NUMPY` — the six magic bytes every `.npy` file opens with.
const MAGIC: &[u8] = b"\x93NUMPY";

/// Largest decompressed `.npz` member this reader will materialise.
///
/// `read_npz_member` reads a whole member into memory (streaming is the follow-up, #458), so without a
/// ceiling a **zip bomb** — a few KiB that inflate to hundreds of GiB — is an OOM-kill triggered by an
/// untrusted file, and the *declared* uncompressed size cannot be used as the guard because it is
/// attacker-controlled in exactly the same way. This is a guard, not a statement about how large an
/// array Tessera can hold: a member beyond it is directed at `ingest blob`, which streams.
#[cfg(feature = "npz")]
const MAX_NPZ_MEMBER_BYTES: u64 = 4 << 30; // 4 GiB

/// What an `.npy` file turned out to contain.
///
/// Two variants because the *shape of the data* decides the primitive (ADR-0056 §1), and a NumPy
/// structured dtype is the one case where an `.npy` file holds a table.
#[derive(Debug)]
pub enum NpyContent {
    /// A dense numeric grid → the array primitive.
    ///
    /// Boxed so the enum stays small: `ArraySpec` carries several `Vec`s and `CanonicalTable` is wider
    /// still, so without the indirection every `NpyContent` would pay the larger variant's size
    /// (`clippy::large_enum_variant`). One allocation per ingested file, against a value that is moved
    /// several times on its way to the seal.
    Array(Box<NpyArray>),
    /// A structured/record dtype → the table primitive.
    Table(Box<CanonicalTable>),
}

/// The array payload of [`NpyContent::Array`].
#[derive(Debug)]
pub struct NpyArray {
    pub spec: ArraySpec,
    pub data: ArrayData,
    /// Recorded transforms (`fortran_to_c_order` when the source was F-ordered).
    pub transforms: Vec<IngestTransform>,
}

/// One field of a NumPy dtype descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct NpyField {
    /// Field name (empty for a plain, unstructured dtype).
    pub name: String,
    /// The fd5 numpy code (`i2`, `u4`, `f8`, `b1`, …) — byte order stripped.
    pub code: String,
    /// Whether the source stored this field big-endian.
    pub big_endian: bool,
    /// Width in bytes of one element.
    pub width: usize,
}

/// A parsed `.npy` header.
#[derive(Debug, Clone, PartialEq)]
pub struct NpyHeader {
    /// One field for a plain dtype; several for a structured one.
    pub fields: Vec<NpyField>,
    /// Whether the buffer is Fortran- (column-major) rather than C-ordered.
    pub fortran_order: bool,
    /// Shape in C-axis order as the header declares it.
    pub shape: Vec<u64>,
    /// Byte offset at which the data buffer begins.
    pub data_offset: usize,
}

impl NpyHeader {
    /// Whether this is a structured (record) dtype — i.e. a table.
    pub fn is_structured(&self) -> bool {
        self.fields.len() > 1 || self.fields.first().is_some_and(|f| !f.name.is_empty())
    }

    /// Elements implied by the shape, or `None` if the product overflows `u64`.
    ///
    /// An empty shape is a 0-d array: **one** element, not zero — hence the fold from 1 rather than a
    /// special case. `Vec::product()` would be wrong here and not merely imprecise: the shape comes
    /// from an untrusted header, and `(2**32, 2**32)` wraps to **zero** in a release build, which made
    /// an empty payload satisfy the length check and sealed an array claiming 2**64 elements with no
    /// data behind it. In a debug build the same input panicked instead. A malformed header has to be a
    /// typed error in both.
    pub fn element_count(&self) -> Option<u64> {
        self.shape
            .iter()
            .try_fold(1u64, |acc, &dim| acc.checked_mul(dim))
    }

    /// Bytes one record occupies (the sum of the field widths).
    pub fn record_width(&self) -> usize {
        self.fields.iter().map(|f| f.width).sum()
    }
}

/// Parse an `.npy` header (v1.0, v2.0 and v3.0).
///
/// The header is a Python dict *literal* — `{'descr': '<f8', 'fortran_order': False, 'shape': (3, 4), }`
/// — which NumPy guarantees is ASCII (v1/v2) or UTF-8 (v3) and built from a fixed, tiny grammar. So it
/// is scanned for the three keys rather than evaluated: `eval`-ing a literal from an untrusted file is
/// how `allow_pickle` bugs happen, and the grammar here is small enough that a scanner cannot
/// mis-read it.
pub fn parse_header(bytes: &[u8]) -> Result<NpyHeader> {
    if bytes.len() < 10 || !bytes.starts_with(MAGIC) {
        return Err(he(
            "not a NumPy .npy file (missing the \\x93NUMPY magic)\n  \
             if it is a .npz archive:  tessera ingest array <FILE> --from npz\n  \
             if it is un-parseable:    tessera ingest blob <FILE>",
        ));
    }
    let (major, minor) = (bytes[6], bytes[7]);
    // v1 uses a 2-byte header length, v2 and v3 use 4. Nothing else is defined.
    let (len_width, header_start) = match major {
        1 => (2usize, 10usize),
        2 | 3 => (4usize, 12usize),
        other => {
            return Err(he(format!(
            "unsupported .npy format version {other}.{minor} (this reader knows 1.x, 2.x and 3.x)"
        )))
        }
    };
    if bytes.len() < header_start {
        return Err(he("truncated .npy header"));
    }
    let header_len = match len_width {
        2 => u16::from_le_bytes([bytes[8], bytes[9]]) as usize,
        _ => u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
    };
    let end = header_start
        .checked_add(header_len)
        .ok_or_else(|| he("header length overflows"))?;
    if bytes.len() < end {
        return Err(he(format!(
            "truncated .npy header: declares {header_len} bytes, file has {}",
            bytes.len().saturating_sub(header_start)
        )));
    }
    let dict = std::str::from_utf8(&bytes[header_start..end])
        .map_err(|e| he(format!("header is not valid UTF-8: {e}")))?;

    let entries = dict_entries(dict)?;
    let value = |key: &str| -> Result<&str> {
        entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
            .ok_or_else(|| he(format!("header has no '{key}' key: {dict}")))
    };

    let fortran_order = match value("fortran_order")?.trim() {
        "True" => true,
        "False" => false,
        other => {
            return Err(he(format!(
                "header 'fortran_order' is '{other}', expected True or False"
            )))
        }
    };
    let shape = parse_shape(value("shape")?)?;
    let fields = parse_descr(value("descr")?)?;

    Ok(NpyHeader {
        fields,
        fortran_order,
        shape,
        data_offset: end,
    })
}

/// Split the header dict literal into its **top-level** `key: value` pairs.
///
/// Parsed rather than searched. Scanning the whole dict for `'shape'` — the previous approach — is wrong
/// in three ways that all matter, because a header is untrusted input and the standard says NumPy will
/// `literal_eval` it:
///
/// 1. **Duplicate keys resolved the wrong way.** A substring search takes the FIRST occurrence; Python
///    keeps the LAST. `{'shape': (2,), 'shape': (999,)}` therefore meant one thing here and another to
///    NumPy — two readers disagreeing about the same bytes, which for a content-addressed format is
///    worse than either answer.
/// 2. **A field named after a key.** `descr` may be a list of `(name, format)` pairs, so
///    `[('shape', '<i4')]` puts the text `'shape'` *inside* another key's value, where a search would
///    find it first.
/// 3. **Unknown keys waved through.** NumPy rejects a header with keys it does not expect; accepting
///    them means a file NumPy refuses to load would still seal here.
///
/// The grammar is tiny and fixed, so this is a scanner over it, not an evaluator — `literal_eval` on an
/// untrusted file is the road to the `allow_pickle` class of bug.
fn dict_entries(dict: &str) -> Result<Vec<(String, &str)>> {
    let body = dict.trim();
    let body = body
        .strip_prefix('{')
        .ok_or_else(|| he(format!("header is not a dict literal: {dict}")))?;
    let body = match body.rfind('}') {
        Some(i) => &body[..i],
        None => return Err(he(format!("header dict is unterminated: {dict}"))),
    };

    let mut out: Vec<(String, &str)> = Vec::new();
    for item in split_top_level(body, b',') {
        let item = item.trim();
        if item.is_empty() {
            continue; // NumPy writes a trailing comma.
        }
        let mut halves = split_top_level(item, b':');
        let raw_key = halves
            .next()
            .ok_or_else(|| he(format!("header entry '{item}' has no key")))?
            .trim();
        let val = halves
            .next()
            .ok_or_else(|| he(format!("header key {raw_key} is not followed by ':'")))?
            .trim();
        if halves.next().is_some() {
            return Err(he(format!("header entry '{item}' has more than one ':'")));
        }
        let key = raw_key
            .strip_prefix('\'')
            .and_then(|k| k.strip_suffix('\''))
            .or_else(|| raw_key.strip_prefix('"').and_then(|k| k.strip_suffix('"')))
            .ok_or_else(|| he(format!("header key {raw_key} is not a quoted string")))?;
        if !matches!(key, "descr" | "fortran_order" | "shape") {
            return Err(he(format!(
                "header has an unexpected key '{key}' — NumPy itself refuses such a header, so \
                 accepting it would seal a file NumPy cannot load\n  \
                 preserve it as-is instead:  tessera ingest blob <FILE>"
            )));
        }
        // Python's dict literal keeps the LAST binding. Rather than silently follow that for a file
        // that is almost certainly hostile or corrupt, refuse: a header with a duplicated key has no
        // single honest reading.
        if out.iter().any(|(k, _)| k == key) {
            return Err(he(format!(
                "header declares '{key}' more than once; Python would keep the last binding, so the \
                 file has no unambiguous reading\n  \
                 preserve it as-is instead:  tessera ingest blob <FILE>"
            )));
        }
        out.push((key.to_string(), val));
    }
    Ok(out)
}

/// Iterate `text` split on `sep`, ignoring separators inside quotes or brackets.
fn split_top_level(text: &str, sep: u8) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_quote: Option<u8> = None;
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match in_quote {
            Some(q) => {
                if b == q {
                    in_quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => in_quote = Some(b),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ if b == sep && depth == 0 => {
                    parts.push(&text[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    parts.push(&text[start..]);
    parts.into_iter()
}

/// Parse a `shape` tuple: `()`, `(5,)`, `(3, 4, 5)`.
fn parse_shape(value: &str) -> Result<Vec<u64>> {
    let inner = value
        .trim()
        .strip_prefix('(')
        .and_then(|v| v.strip_suffix(')'))
        .ok_or_else(|| he(format!("header 'shape' is not a tuple: {value}")))?;
    inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<u64>().map_err(|e| {
                he(format!(
                    "header 'shape' has a non-integer dimension '{s}': {e}"
                ))
            })
        })
        .collect()
}

/// Parse a `descr`: either a single dtype string, or a list of `(name, dtype)` tuples.
fn parse_descr(value: &str) -> Result<Vec<NpyField>> {
    let value = value.trim();
    if value.starts_with('[') {
        // Structured: `[('x', '<i4'), ('y', '<f8')]`. Each tuple's two quoted strings are the name
        // and the dtype; a third element (a sub-shape) means a nested array field, which has no flat
        // column representation.
        let inner = value
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .ok_or_else(|| he(format!("header 'descr' list is unterminated: {value}")))?;
        let mut fields = Vec::new();
        for tuple in split_tuples(inner) {
            // Count the tuple's TOP-LEVEL elements, not its quoted strings. `('x', '<i4', (3,))` has
            // only two quoted strings, so counting those silently dropped the `(3,)` sub-shape and
            // sealed the field as a plain `<i4` — losing three quarters of every record. A sub-array
            // field has no flat column representation and must be refused, not quietly flattened.
            let elements = split_top_level(&tuple, b',')
                .filter(|e| !e.trim().is_empty())
                .count();
            if elements != 2 {
                return Err(he(format!(
                    "structured dtype field '({tuple})' has {elements} elements, not a (name, dtype) \
                     pair — a sub-array field like ('x', '<i4', (3,)) has no flat column \
                     representation\n  \
                     keep the bytes: tessera ingest blob <FILE>"
                )));
            }
            let parts = quoted_strings(&tuple);
            match parts.len() {
                2 => {
                    let mut f = parse_dtype_string(&parts[1])?;
                    f.name = parts[0].clone();
                    if f.name.is_empty() {
                        return Err(he("a structured dtype field has an empty name"));
                    }
                    fields.push(f);
                }
                _ => {
                    return Err(he(format!(
                        "structured dtype field '{tuple}' is not a (name, dtype) pair — nested \
                         sub-array fields have no flat column representation\n  \
                         keep the bytes: tessera ingest blob <FILE>"
                    )))
                }
            }
        }
        if fields.is_empty() {
            return Err(he("header 'descr' is an empty field list"));
        }
        return Ok(fields);
    }
    let unquoted = value.trim_matches(|c| c == '\'' || c == '"');
    Ok(vec![parse_dtype_string(unquoted)?])
}

/// Split a `descr` list body into its top-level `(...)` tuples.
fn split_tuples(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in inner.chars() {
        match c {
            '(' => {
                depth += 1;
                if depth == 1 {
                    cur.clear();
                    continue;
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    out.push(std::mem::take(&mut cur));
                    continue;
                }
            }
            _ => {}
        }
        if depth >= 1 {
            cur.push(c);
        }
    }
    out
}

/// The quoted strings inside one `descr` tuple, in order.
fn quoted_strings(tuple: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = tuple.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            let mut s = String::new();
            for d in chars.by_ref() {
                if d == c {
                    break;
                }
                s.push(d);
            }
            out.push(s);
        }
    }
    out
}

/// Parse one NumPy dtype string — `'<f8'`, `'>i2'`, `'|u1'`, `'=f4'`, `'b1'`.
///
/// The leading character is the byte order (`<` little, `>` big, `|` not-applicable, `=` native) and
/// is optional. What follows is a kind letter and a byte width.
fn parse_dtype_string(descr: &str) -> Result<NpyField> {
    let descr = descr.trim();
    let (order, rest) = match descr.chars().next() {
        Some(c @ ('<' | '>' | '|' | '=')) => (c, &descr[1..]),
        _ => ('|', descr),
    };
    // Native order is little-endian on every architecture Tessera targets; `cfg!` keeps that honest
    // rather than assumed, so a big-endian host would read `=` correctly too.
    let big_endian = order == '>' || (order == '=' && cfg!(target_endian = "big"));

    let mut kind_chars = rest.chars();
    let kind = kind_chars
        .next()
        .ok_or_else(|| he(format!("dtype '{descr}' has no kind letter")))?;
    let width_text: String = kind_chars.collect();

    // Rejected outright, each with the reason and the escape hatch.
    match kind {
        'O' => {
            return Err(he(
                "dtype is 'object' — a pickled .npy is arbitrary Python code, not data, and Tessera \
                 will not unpickle it (there is no `allow_pickle` here, by design)\n  \
                 keep every byte: tessera ingest blob <FILE>",
            ))
        }
        'U' | 'S' | 'a' => {
            return Err(he(format!(
                "dtype '{descr}' is a fixed-width string; Tessera's ARRAY primitive is a numeric grid\n  \
                 for text, use a table:  tessera ingest table <FILE>  (a structured dtype routes there \
                 automatically)\n  \
                 or keep the bytes:      tessera ingest blob <FILE>"
            )))
        }
        'M' | 'm' => {
            return Err(he(format!(
                "dtype '{descr}' is a datetime64/timedelta64; an array block carries no epoch, so the \
                 instant would be indistinguishable from an elapsed count (ADR-0046)\n  \
                 use a table, where a column can carry an epoch:  tessera ingest table <FILE>"
            )))
        }
        'c' => {
            return Err(he(format!(
                "dtype '{descr}' is complex; Tessera has no complex dtype yet (tracked as #419)\n  \
                 split it:        store the real and imaginary parts as two arrays\n  \
                 keep the bytes:  tessera ingest blob <FILE>"
            )))
        }
        _ => {}
    }

    let width: usize = if width_text.is_empty() {
        return Err(he(format!("dtype '{descr}' has no byte width")));
    } else {
        width_text
            .parse()
            .map_err(|e| he(format!("dtype '{descr}' has a non-numeric width: {e}")))?
    };

    let code = match (kind, width) {
        ('i', 1) => "i1",
        ('i', 2) => "i2",
        ('i', 4) => "i4",
        ('i', 8) => "i8",
        ('u', 1) => "u1",
        ('u', 2) => "u2",
        ('u', 4) => "u4",
        ('u', 8) => "u8",
        ('f', 2) => "f2",
        ('f', 4) => "f4",
        ('f', 8) => "f8",
        ('b', 1) => "b1",
        _ => {
            return Err(he(format!(
            "dtype '{descr}' has no Tessera equivalent (accepted: int/uint 1·2·4·8, float 2·4·8, \
                 bool)\n  keep the bytes: tessera ingest blob <FILE>"
        )))
        }
    };
    Ok(NpyField {
        name: String::new(),
        code: code.to_string(),
        big_endian,
        width,
    })
}

/// **H6** — byte-swap a big-endian buffer to native, in place, `width` bytes at a time.
///
/// The hazard rated *low × fatal*: a naive `cast_slice` of a `>f8` buffer on a little-endian host
/// produces garbage that looks like data. Swapping here, before the waist, is what makes a `>f8` file
/// and its `<f8` twin seal to the same `content_hash` — and note this is deliberately **not** a
/// recorded transform (see the module docs): byte order needs no recovery instructions, and recording
/// it would make the twins differ in the seal, which is the thing the fixture forbids.
fn swap_to_native(buf: &mut [u8], width: usize) {
    if width < 2 {
        return;
    }
    for chunk in buf.chunks_exact_mut(width) {
        chunk.reverse();
    }
}

/// Read an `.npy` file into whichever primitive its dtype implies.
pub fn read_npy(path: &Path) -> Result<NpyContent> {
    let bytes = std::fs::read(path).map_err(|e| he(format!("read {}: {e}", path.display())))?;
    read_npy_bytes(&bytes, &path.display().to_string())
}

/// [`read_npy`] over an in-memory buffer — the seam the `.npz` members go through.
pub fn read_npy_bytes(bytes: &[u8], label: &str) -> Result<NpyContent> {
    let header = parse_header(bytes)?;
    let elements = header.element_count().ok_or_else(|| {
        he(format!(
            "{label}: declared shape {:?} overflows u64 when multiplied out",
            header.shape
        ))
    })?;
    let payload = &bytes[header.data_offset..];
    let expected = usize::try_from(elements)
        .ok()
        .and_then(|n| n.checked_mul(header.record_width()))
        .ok_or_else(|| he("declared shape overflows a host-sized buffer"))?;
    if payload.len() != expected {
        return Err(he(format!(
            "{label}: shape {:?} × {} bytes/record = {expected} bytes, but the file carries {} — \
             the file is truncated or the header disagrees with the data",
            header.shape,
            header.record_width(),
            payload.len()
        )));
    }

    if header.is_structured() {
        return Ok(NpyContent::Table(Box::new(structured_to_table(
            &header, payload,
        )?)));
    }

    let field = &header.fields[0];
    let mut buf = payload.to_vec();
    if field.big_endian {
        swap_to_native(&mut buf, field.width);
    }
    let mut data = ArrayData::from_le_bytes(&field.code, &buf)?;

    let mut transforms = Vec::new();
    if header.fortran_order && header.shape.len() > 1 {
        // A 1-D or 0-d array is the same buffer either way, so only a genuine reordering is recorded.
        data = fortran_to_c(&data, &header.shape)?;
        transforms.push(
            IngestTransform::new(transform::FORTRAN_TO_C_ORDER)
                .with("shape", serde_json::json!(header.shape)),
        );
    }

    let spec = ArraySpec::new(header.shape.clone(), data.dtype());
    Ok(NpyContent::Array(Box::new(NpyArray {
        spec,
        data,
        transforms,
    })))
}

/// Reorder a Fortran-ordered (column-major) buffer into C order (row-major).
///
/// Tessera arrays are C-order by definition — `ArraySpec.shape` and `tessera_io::array`'s chunk walk
/// both assume it — so an F-ordered source must be rewritten rather than relabelled. Relabelling by
/// reversing the shape would make the *axes* wrong instead of the bytes, which is worse: the data
/// would read back transposed with no record of why.
fn fortran_to_c(data: &ArrayData, shape: &[u64]) -> Result<ArrayData> {
    let dims: Vec<usize> = shape
        .iter()
        .map(|&d| usize::try_from(d).map_err(|_| he("dimension overflows usize")))
        .collect::<Result<_>>()?;
    let total: usize = dims.iter().product();
    if total != data.len() {
        return Err(he(format!(
            "internal: {} elements for shape {:?}",
            data.len(),
            shape
        )));
    }
    // For each C-order position, compute where that element sits in the F-order buffer. Walking the
    // destination (rather than the source) keeps the gather a simple permutation with no scatter.
    let rank = dims.len();
    let mut f_strides = vec![1usize; rank];
    for i in 1..rank {
        f_strides[i] = f_strides[i - 1] * dims[i - 1];
    }
    let mut index = vec![0usize; rank];
    let mut order = Vec::with_capacity(total);
    for _ in 0..total {
        order.push(
            index
                .iter()
                .zip(&f_strides)
                .map(|(i, s)| i * s)
                .sum::<usize>(),
        );
        // Increment the C-order odometer: last axis fastest.
        for axis in (0..rank).rev() {
            index[axis] += 1;
            if index[axis] < dims[axis] {
                break;
            }
            index[axis] = 0;
        }
    }
    Ok(permute(data, &order))
}

/// Gather `data` in the given source order.
fn permute(data: &ArrayData, order: &[usize]) -> ArrayData {
    macro_rules! take {
        ($v:expr, $variant:path) => {
            $variant(order.iter().map(|&i| $v[i]).collect())
        };
    }
    match data {
        ArrayData::I8(v) => take!(v, ArrayData::I8),
        ArrayData::U8(v) => take!(v, ArrayData::U8),
        ArrayData::I16(v) => take!(v, ArrayData::I16),
        ArrayData::I32(v) => take!(v, ArrayData::I32),
        ArrayData::I64(v) => take!(v, ArrayData::I64),
        ArrayData::U16(v) => take!(v, ArrayData::U16),
        ArrayData::U32(v) => take!(v, ArrayData::U32),
        ArrayData::U64(v) => take!(v, ArrayData::U64),
        ArrayData::F16(v) => take!(v, ArrayData::F16),
        ArrayData::F32(v) => take!(v, ArrayData::F32),
        ArrayData::F64(v) => take!(v, ArrayData::F64),
        ArrayData::Bool(v) => take!(v, ArrayData::Bool),
    }
}

/// A structured/record `.npy` → a flat table (ADR-0056 §1/§11).
///
/// NumPy stores a record array **interleaved**: all of record 0's fields, then all of record 1's. So
/// this is a de-interleave — the same transpose the GE-HDF5 compound reader performs (#193), and for
/// the same reason: a single-column projection on an interleaved layout costs a full read.
fn structured_to_table(header: &NpyHeader, payload: &[u8]) -> Result<CanonicalTable> {
    use crate::canonical::{canonicalise, unclassified_column, TableBuilder};
    use tessera_io::table::ColumnData;

    if header.shape.len() != 1 {
        return Err(he(format!(
            "a structured dtype with shape {:?} is a {}-D array OF records; Tessera tables are flat, \
             so only a 1-D record array maps to one\n  keep the bytes: tessera ingest blob <FILE>",
            header.shape,
            header.shape.len()
        )));
    }
    let rows = usize::try_from(header.shape[0]).map_err(|_| he("row count overflows usize"))?;
    let stride = header.record_width();
    let mut b = TableBuilder::new();
    // De-interleave: for each field, walk its slot in every record.
    let mut offset = 0usize;
    for field in &header.fields {
        let mut buf = Vec::with_capacity(rows * field.width);
        for r in 0..rows {
            let start = r * stride + offset;
            buf.extend_from_slice(&payload[start..start + field.width]);
        }
        if field.big_endian {
            swap_to_native(&mut buf, field.width);
        }
        let mut data = ColumnData::from_le_bytes(&field.code, &buf)?;
        canonicalise(&mut b, &field.name, &mut data);
        b.push(unclassified_column(&field.name, &field.code), data)?;
        offset += field.width;
    }
    let mut table = b.finish();
    table
        .transforms
        .push(IngestTransform::new(transform::RECORD_DEINTERLEAVE));
    table.transforms.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_io::table::ColumnData;

    /// Build an `.npy` file in memory. A writer in the tests rather than committed binary fixtures,
    /// for ADR-0056 §5's reason: a committed `.npy` would be a binary blob in the repo whose contents
    /// nobody can review, and generating it makes the *inputs* legible.
    fn npy(
        descr: &str,
        fortran: bool,
        shape: &[u64],
        payload: &[u8],
        version: (u8, u8),
    ) -> Vec<u8> {
        let shape_text = if shape.len() == 1 {
            format!("({},)", shape[0])
        } else {
            format!(
                "({})",
                shape
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let dict = format!(
            "{{'descr': {descr}, 'fortran_order': {}, 'shape': {shape_text}, }}",
            if fortran { "True" } else { "False" }
        );
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(version.0);
        out.push(version.1);
        // NumPy pads the header with spaces so the data starts 64-byte aligned; the reader must not
        // depend on that, so the tests exercise both padded and unpadded headers.
        let len_width = if version.0 == 1 { 2 } else { 4 };
        let header = format!("{dict}\n");
        let total = MAGIC.len() + 2 + len_width + header.len();
        let pad = (64 - (total % 64)) % 64;
        let header = format!("{}{}\n", dict, " ".repeat(pad));
        if len_width == 2 {
            out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        } else {
            out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A `.npy` with an arbitrary header dict, for the header-grammar tests.
    fn raw_npy(dict: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(1);
        out.push(0);
        let header = format!("{dict}\n");
        out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn f64_le(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn f64_be(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    fn array_of(content: NpyContent) -> (ArraySpec, ArrayData, Vec<IngestTransform>) {
        match content {
            NpyContent::Array(a) => (a.spec, a.data, a.transforms),
            NpyContent::Table(_) => panic!("expected an array, got a table"),
        }
    }

    /// **#461 nit.** A sub-array field — `('x', '<i4', (3,))`, three `int32`s per record — was
    /// *accepted* as a plain `<i4`, because the guard counted the tuple's **quoted strings** (two of
    /// them) rather than its elements. The `(3,)` was silently dropped, so the field claimed one value
    /// per record where the file holds three: every record after the first would be misaligned, and the
    /// product would seal wrong data and verify.
    #[test]
    fn a_sub_array_field_is_refused_not_silently_flattened() {
        let dict = "{'descr': [('x', '<i4', (3,)), ('y', '<f8')], 'fortran_order': False, 'shape': (2,), }";
        let err = parse_header(&raw_npy(dict, &[0u8; 40]))
            .expect_err("a sub-array field must be refused")
            .to_string();
        assert!(
            err.contains("sub-array") || err.contains("elements"),
            "got: {err}"
        );

        // The ordinary (name, dtype) pair is untouched.
        let dict =
            "{'descr': [('x', '<i4'), ('y', '<f8')], 'fortran_order': False, 'shape': (2,), }";
        let h = parse_header(&raw_npy(dict, &[0u8; 24])).expect("a plain pair still parses");
        assert_eq!(h.fields.len(), 2);
    }

    /// **#461 blocker 2.** The header keys were found by searching the whole dict for `'shape'` etc.
    /// Three consequences, all of them reachable from a crafted file:
    ///
    /// - a duplicated key resolved to the FIRST binding here and the LAST in Python, so the same bytes
    ///   meant different things to us and to NumPy;
    /// - a structured `descr` containing a field literally named `shape` put that text inside another
    ///   key's value, where the search found it first;
    /// - unknown keys were accepted, so a header NumPy refuses to load would still seal.
    ///
    /// The dict is now parsed at the top level instead, and all three are refused.
    #[test]
    fn a_header_key_is_parsed_at_the_top_level_not_searched_for() {
        // A field named after a key must not be mistaken for the key. `descr` comes FIRST here, which
        // is exactly the order that made the old search read the field name as the shape.
        let dict =
            "{'descr': [('shape', '<i4'), ('x', '<f8')], 'fortran_order': False, 'shape': (2,), }";
        let h = parse_header(&raw_npy(dict, &[0u8; 24])).expect("a field may be named 'shape'");
        assert_eq!(h.shape, vec![2]);
        assert_eq!(h.fields.len(), 2);
        assert_eq!(h.fields[0].name, "shape");

        // A duplicated key has no single honest reading — Python keeps the last, a search kept the
        // first — so it is refused rather than silently resolved either way.
        let dict = "{'descr': '<f8', 'fortran_order': False, 'shape': (2,), 'shape': (999,), }";
        let err = parse_header(&raw_npy(dict, &[0u8; 16]))
            .expect_err("a duplicated key must be refused")
            .to_string();
        assert!(err.contains("more than once"), "got: {err}");

        // An unexpected key is refused, because NumPy refuses it too.
        let dict = "{'descr': '<f8', 'fortran_order': False, 'shape': (2,), 'evil': 1, }";
        let err = parse_header(&raw_npy(dict, &[0u8; 16]))
            .expect_err("an unknown key must be refused")
            .to_string();
        assert!(err.contains("unexpected key 'evil'"), "got: {err}");

        // Double-quoted keys and a missing trailing comma are both legal and must still parse.
        let dict = "{\"descr\": \"<f8\", \"fortran_order\": False, \"shape\": (2,)}";
        let h = parse_header(&raw_npy(dict, &[0u8; 16])).expect("double-quoted keys are legal");
        assert_eq!(h.shape, vec![2]);
    }

    /// **#461 blocker 1.** `element_count()` multiplied the shape with `Vec::product()`, which wraps
    /// in release and panics in debug. A header declaring `(2**32, 2**32)` therefore reported **0**
    /// elements in a release build, so `expected` was 0 bytes and a file with an empty payload passed
    /// the length check — sealing an array that *claims* 2**64 elements while carrying none. In debug
    /// the same input panicked, which is the other half of the bug: a malformed file must be a typed
    /// error either way (the same rule #447 applied to the NIfTI header).
    #[test]
    fn a_shape_that_overflows_u64_is_a_typed_error_not_a_wrap_or_a_panic() {
        // 2**32 * 2**32 == 2**64, which wraps to 0.
        let bytes = npy("'<f8'", false, &[1 << 32, 1 << 32], &[], (1, 0));
        let err = read_npy_bytes(&bytes, "bomb")
            .expect_err("a shape whose product overflows u64 must be rejected")
            .to_string();
        assert!(
            err.contains("overflow"),
            "the error should name the overflow, got: {err}"
        );

        // The boundary immediately below is legitimate arithmetic and must still be rejected for the
        // ordinary reason — the payload cannot possibly be there — not for overflow.
        let bytes = npy("'<f8'", false, &[1 << 32, 1 << 31], &[], (1, 0));
        let err = read_npy_bytes(&bytes, "huge")
            .expect_err("a shape that does not overflow but cannot be backed must still fail")
            .to_string();
        assert!(
            err.contains("overflows a host-sized buffer") || err.contains("carries 0"),
            "got: {err}"
        );

        // And a 0-d array is still ONE element, which the fold must not turn into zero.
        let bytes = npy("'<f8'", false, &[], &f64_le(&[42.0]), (1, 0));
        assert!(read_npy_bytes(&bytes, "scalar").is_ok());
    }

    #[test]
    fn a_plain_c_order_npy_reads_as_an_array() {
        let values = vec![1.5f64, -0.5, 7.25, 0.0, f64::MIN_POSITIVE, -0.0];
        let bytes = npy("'<f8'", false, &[2, 3], &f64_le(&values), (1, 0));
        let (spec, data, transforms) = array_of(read_npy_bytes(&bytes, "t").unwrap());
        assert_eq!(spec.shape, vec![2, 3]);
        assert_eq!(spec.dtype, "float64");
        assert_eq!(data, ArrayData::F64(values));
        assert!(transforms.is_empty(), "a C-order read transforms nothing");
    }

    /// **H6** — the fixture ADR-0056 §5 names: a `>f8` file and its `<f8` twin must produce the same
    /// values. A naive `cast_slice` of the big-endian buffer would produce garbage that looks like data.
    #[test]
    fn a_big_endian_npy_and_its_little_endian_twin_read_identically() {
        let values = vec![1.5f64, -0.5, 7.25, 1e-300];
        let le = npy("'<f8'", false, &[4], &f64_le(&values), (1, 0));
        let be = npy("'>f8'", false, &[4], &f64_be(&values), (1, 0));
        assert_ne!(le, be, "the two files really are different bytes");

        let (_, le_data, le_t) = array_of(read_npy_bytes(&le, "le").unwrap());
        let (_, be_data, be_t) = array_of(read_npy_bytes(&be, "be").unwrap());
        assert_eq!(le_data, be_data);
        assert_eq!(le_data, ArrayData::F64(values));
        // Byte order is NOT a recorded transform: it needs no recovery instructions, and recording it
        // would make the twins differ in the seal — which is what the §5 fixture forbids.
        assert!(le_t.is_empty() && be_t.is_empty());
    }

    /// Every accepted width, in both byte orders, must agree — the widths are where a byte-swap bug
    /// hides (a 2-byte swap that ran over 4-byte elements would pass the f8 test above).
    #[test]
    fn every_accepted_dtype_round_trips_in_both_byte_orders() {
        // (descr kind+width, little-endian payload, expected)
        let cases: Vec<(&str, Vec<u8>, ArrayData)> = vec![
            ("i1", vec![0xff, 0x01], ArrayData::I8(vec![-1, 1])),
            ("u1", vec![0xff, 0x01], ArrayData::U8(vec![255, 1])),
            (
                "i2",
                (-2i16)
                    .to_le_bytes()
                    .into_iter()
                    .chain(300i16.to_le_bytes())
                    .collect(),
                ArrayData::I16(vec![-2, 300]),
            ),
            (
                "u2",
                65535u16
                    .to_le_bytes()
                    .into_iter()
                    .chain(7u16.to_le_bytes())
                    .collect(),
                ArrayData::U16(vec![65535, 7]),
            ),
            (
                "i4",
                i32::MIN
                    .to_le_bytes()
                    .into_iter()
                    .chain(5i32.to_le_bytes())
                    .collect(),
                ArrayData::I32(vec![i32::MIN, 5]),
            ),
            (
                "u4",
                u32::MAX
                    .to_le_bytes()
                    .into_iter()
                    .chain(5u32.to_le_bytes())
                    .collect(),
                ArrayData::U32(vec![u32::MAX, 5]),
            ),
            (
                "i8",
                i64::MIN
                    .to_le_bytes()
                    .into_iter()
                    .chain(5i64.to_le_bytes())
                    .collect(),
                ArrayData::I64(vec![i64::MIN, 5]),
            ),
            (
                "u8",
                u64::MAX
                    .to_le_bytes()
                    .into_iter()
                    .chain(5u64.to_le_bytes())
                    .collect(),
                ArrayData::U64(vec![u64::MAX, 5]),
            ),
            (
                "f4",
                (1.5f32)
                    .to_le_bytes()
                    .into_iter()
                    .chain((-0.25f32).to_le_bytes())
                    .collect(),
                ArrayData::F32(vec![1.5, -0.25]),
            ),
            (
                "f8",
                f64_le(&[1.5, -0.25]),
                ArrayData::F64(vec![1.5, -0.25]),
            ),
            ("b1", vec![0x01, 0x00], ArrayData::Bool(vec![true, false])),
        ];
        for (code, le_payload, want) in cases {
            let width: usize = code[1..].parse().unwrap();
            let le = npy(&format!("'<{code}'"), false, &[2], &le_payload, (1, 0));
            let (_, got, _) = array_of(read_npy_bytes(&le, code).unwrap());
            assert_eq!(got, want, "little-endian '{code}'");

            // The big-endian twin: same values, bytes reversed per element.
            let mut be_payload = le_payload.clone();
            for chunk in be_payload.chunks_exact_mut(width) {
                chunk.reverse();
            }
            let be = npy(&format!("'>{code}'"), false, &[2], &be_payload, (1, 0));
            let (_, got, _) = array_of(read_npy_bytes(&be, code).unwrap());
            assert_eq!(got, want, "big-endian '{code}'");
        }
    }

    /// Fortran order is a genuine reordering of the buffer, so it is rewritten AND recorded. Relabelling
    /// by reversing the shape would make the axes wrong instead of the bytes — worse, because the data
    /// would read back transposed with no record of why.
    #[test]
    fn a_fortran_ordered_npy_is_rewritten_to_c_order_and_recorded() {
        // The 2×3 matrix [[1,2,3],[4,5,6]]. C order is 1,2,3,4,5,6; F order is 1,4,2,5,3,6.
        let f_order = vec![1.0f64, 4.0, 2.0, 5.0, 3.0, 6.0];
        let bytes = npy("'<f8'", true, &[2, 3], &f64_le(&f_order), (1, 0));
        let (spec, data, transforms) = array_of(read_npy_bytes(&bytes, "f").unwrap());
        assert_eq!(spec.shape, vec![2, 3], "the shape is NOT reversed");
        assert_eq!(
            data,
            ArrayData::F64(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            "the buffer is rewritten to C order"
        );
        let rec = transforms
            .iter()
            .find(|t| t.name == transform::FORTRAN_TO_C_ORDER)
            .expect("recorded");
        assert_eq!(rec.params["shape"], serde_json::json!([2, 3]));

        // …and the C-order file of the same logical matrix produces the identical array, which is the
        // property that matters: the producer's memory layout does not reach the payload.
        let c = npy(
            "'<f8'",
            false,
            &[2, 3],
            &f64_le(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            (1, 0),
        );
        let (_, c_data, _) = array_of(read_npy_bytes(&c, "c").unwrap());
        assert_eq!(data, c_data);
    }

    /// A 3-D Fortran transpose — a rank-2 test can pass with the axes accidentally swapped.
    #[test]
    fn the_fortran_transpose_is_correct_at_rank_three() {
        // shape [2,2,2]; C order values 0..8 laid out as F order.
        let dims = [2usize, 2, 2];
        let mut f_order = vec![0f64; 8];
        for z in 0..dims[0] {
            for y in 0..dims[1] {
                for x in 0..dims[2] {
                    let c_index = (z * dims[1] + y) * dims[2] + x;
                    let f_index = z + dims[0] * (y + dims[1] * x);
                    f_order[f_index] = c_index as f64;
                }
            }
        }
        let bytes = npy("'<f8'", true, &[2, 2, 2], &f64_le(&f_order), (1, 0));
        let (_, data, _) = array_of(read_npy_bytes(&bytes, "f3").unwrap());
        assert_eq!(
            data,
            ArrayData::F64((0..8).map(|i| i as f64).collect()),
            "the C-order read must recover 0..8 in order"
        );
    }

    /// A 1-D Fortran array is the same buffer either way, so nothing is rewritten and nothing recorded —
    /// otherwise the seal would claim a transform that changed nothing.
    #[test]
    fn a_one_dimensional_fortran_array_records_nothing() {
        let bytes = npy("'<f8'", true, &[3], &f64_le(&[1.0, 2.0, 3.0]), (1, 0));
        let (_, data, transforms) = array_of(read_npy_bytes(&bytes, "f1").unwrap());
        assert_eq!(data, ArrayData::F64(vec![1.0, 2.0, 3.0]));
        assert!(transforms.is_empty());
    }

    /// ADR-0056 §1 in its purest form: a structured dtype is rows of typed fields, so it is a TABLE.
    /// NumPy stores records interleaved, so this is the #193 transpose one format over.
    #[test]
    fn a_structured_dtype_routes_to_the_table_primitive() {
        // Two records of (i4, f8): (1, 1.5) and (-2, -0.25), interleaved.
        let mut payload = Vec::new();
        for (i, f) in [(1i32, 1.5f64), (-2, -0.25)] {
            payload.extend_from_slice(&i.to_le_bytes());
            payload.extend_from_slice(&f.to_le_bytes());
        }
        let bytes = npy(
            "[('id', '<i4'), ('energy', '<f8')]",
            false,
            &[2],
            &payload,
            (1, 0),
        );
        let NpyContent::Table(t) = read_npy_bytes(&bytes, "s").unwrap() else {
            panic!("a structured dtype must route to the table primitive")
        };
        assert_eq!(t.rows(), 2);
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "energy"]
        );
        assert_eq!(t.columns[0].1, ColumnData::I32(vec![1, -2]));
        assert_eq!(t.columns[1].1, ColumnData::F64(vec![1.5, -0.25]));
        assert!(t
            .transforms
            .iter()
            .any(|x| x.name == transform::RECORD_DEINTERLEAVE));
        // Unclassified, like every generic column.
        for (c, _) in &t.columns {
            assert_eq!(c.sensitivity, tessera_core::schema::Sensitivity::Unknown);
        }
    }

    /// The de-interleave must handle a big-endian record too — the swap is per field, at that field's
    /// width, not over the whole record.
    #[test]
    fn a_big_endian_structured_dtype_de_interleaves_correctly() {
        let mut payload = Vec::new();
        for (i, f) in [(1i32, 1.5f64), (-2, -0.25)] {
            payload.extend_from_slice(&i.to_be_bytes());
            payload.extend_from_slice(&f.to_be_bytes());
        }
        let bytes = npy(
            "[('id', '>i4'), ('energy', '>f8')]",
            false,
            &[2],
            &payload,
            (1, 0),
        );
        let NpyContent::Table(t) = read_npy_bytes(&bytes, "s").unwrap() else {
            panic!("expected a table")
        };
        assert_eq!(t.columns[0].1, ColumnData::I32(vec![1, -2]));
        assert_eq!(t.columns[1].1, ColumnData::F64(vec![1.5, -0.25]));
    }

    /// **An object dtype is never unpickled.** `allow_pickle` is a remote-code-execution switch and
    /// Tessera does not have one; the error routes to the tier that preserves bytes without
    /// interpreting them.
    #[test]
    fn an_object_dtype_is_refused_and_never_unpickled() {
        let bytes = npy("'|O'", false, &[2], &[0u8; 16], (1, 0));
        let err = read_npy_bytes(&bytes, "o").unwrap_err().to_string();
        assert!(err.contains("arbitrary Python code"), "got {err}");
        assert!(
            err.contains("allow_pickle"),
            "names the switch we do not have: {err}"
        );
        assert!(
            err.contains("ingest blob"),
            "offers the preserve tier: {err}"
        );
    }

    /// Each rejected dtype routes somewhere real rather than just failing.
    #[test]
    fn dtypes_with_no_array_equivalent_are_routed_not_merely_rejected() {
        for (descr, width, needle, route) in [
            ("'<U4'", 16, "fixed-width string", "ingest table"),
            ("'<M8'", 8, "carries no epoch", "ingest table"),
            ("'<c16'", 16, "complex", "ingest blob"),
            ("'<f3'", 3, "no Tessera equivalent", "ingest blob"),
        ] {
            let bytes = npy(descr, false, &[1], &vec![0u8; width], (1, 0));
            let err = read_npy_bytes(&bytes, descr).unwrap_err().to_string();
            assert!(err.contains(needle), "expected '{needle}' in: {err}");
            assert!(
                err.contains(route),
                "expected a route to '{route}' in: {err}"
            );
        }
    }

    #[test]
    fn all_three_header_versions_parse() {
        let values = vec![1.5f64, 2.5];
        for version in [(1u8, 0u8), (2, 0), (3, 0)] {
            let bytes = npy("'<f8'", false, &[2], &f64_le(&values), version);
            let (_, data, _) = array_of(read_npy_bytes(&bytes, "v").unwrap());
            assert_eq!(data, ArrayData::F64(values.clone()), "version {version:?}");
        }
        // An undefined version is refused rather than guessed at.
        let mut bad = npy("'<f8'", false, &[2], &f64_le(&values), (1, 0));
        bad[6] = 9;
        let err = read_npy_bytes(&bad, "v9").unwrap_err().to_string();
        assert!(
            err.contains("unsupported .npy format version 9"),
            "got {err}"
        );
    }

    #[test]
    fn a_zero_dimensional_array_is_one_element_not_zero() {
        // `np.save` of a scalar writes shape ().
        let bytes = npy("'<f8'", false, &[], &f64_le(&[42.0]), (1, 0));
        let (spec, data, _) = array_of(read_npy_bytes(&bytes, "0d").unwrap());
        assert!(spec.shape.is_empty());
        assert_eq!(data, ArrayData::F64(vec![42.0]));
    }

    #[test]
    fn a_truncated_or_mismatched_file_is_an_error_not_a_short_read() {
        // Declares 4 elements, carries 2.
        let bytes = npy("'<f8'", false, &[4], &f64_le(&[1.0, 2.0]), (1, 0));
        let err = read_npy_bytes(&bytes, "short").unwrap_err().to_string();
        assert!(
            err.contains("truncated or the header disagrees"),
            "got {err}"
        );

        // Not an npy at all.
        let err = read_npy_bytes(b"PAR1 not numpy", "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing the"), "got {err}");
        assert!(
            err.contains("--from npz"),
            "points at the archive case: {err}"
        );
    }

    /// The header is scanned, never evaluated — so a dict whose keys arrive in an unusual order, with
    /// double quotes, or with extra padding, still parses.
    #[test]
    fn the_header_scanner_tolerates_real_world_dict_spellings() {
        let payload = f64_le(&[1.0, 2.0]);
        for dict in [
            "{'descr': '<f8', 'fortran_order': False, 'shape': (2,), }",
            "{'shape': (2,), 'descr': '<f8', 'fortran_order': False}",
            "{\"descr\": \"<f8\", \"fortran_order\": False, \"shape\": (2,)}",
            "{'descr':'<f8','fortran_order':False,'shape':(2,)}",
        ] {
            let mut out = Vec::new();
            out.extend_from_slice(MAGIC);
            out.extend_from_slice(&[1, 0]);
            let header = format!("{dict}\n");
            out.extend_from_slice(&(header.len() as u16).to_le_bytes());
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(&payload);
            let (_, data, _) =
                array_of(read_npy_bytes(&out, dict).unwrap_or_else(|e| panic!("{dict}: {e}")));
            assert_eq!(data, ArrayData::F64(vec![1.0, 2.0]), "{dict}");
        }
    }

    /// A structured dtype over a multi-dimensional shape is an N-D array OF records, which a flat table
    /// cannot express — refused rather than silently flattened.
    #[test]
    fn a_multidimensional_record_array_is_refused() {
        let payload = vec![0u8; 4 * 12];
        let bytes = npy(
            "[('a', '<i4'), ('b', '<f8')]",
            false,
            &[2, 2],
            &payload,
            (1, 0),
        );
        let err = read_npy_bytes(&bytes, "nd").unwrap_err().to_string();
        assert!(err.contains("2-D array OF records"), "got {err}");
        assert!(err.contains("ingest blob"), "got {err}");
    }
}

/// The members of a `.npz` archive, in the order the archive lists them.
///
/// A `.npz` is a zip of `.npy` files, so ADR-0056 §11 makes it a **collection** — one array product per
/// member — rather than one product with N blocks. That is §10's shape rule applied one level up: the
/// members of an `.npz` are independent arrays (`np.savez(f, a=…, b=…)`), not slices of one grid.
///
/// Listing is separate from reading so the CLI can expand an archive into an explicit **multi-product
/// spec**, naming each member. That keeps the declarative path the single source of truth (ADR-0035) and
/// makes the spec an honest record of exactly which members were ingested — rather than a spec that says
/// "everything in this archive", whose meaning would depend on the archive.
#[cfg(feature = "npz")]
pub fn npz_members(path: &Path) -> Result<Vec<String>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| {
        he(format!(
            "{} is not a readable .npz archive: {e}\n  \
             if it is a single array:  tessera ingest array <FILE> --from npy\n  \
             if it is un-parseable:    tessera ingest blob <FILE>",
            path.display()
        ))
    })?;
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| he(format!("{}: member {i}: {e}", path.display())))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        check_member_name(path, &name)?;
        // No duplicate-name check here on purpose: `ZipArchive` already collapses duplicate entries
        // to one, so a hand-crafted archive with two `x.npy` members is reported as a single member
        // and there is nothing ambiguous left to refuse. A guard would be unreachable code pretending
        // to be a defence; the behaviour we depend on is pinned by
        // `a_duplicated_npz_member_name_collapses_in_the_zip_reader` instead.
        names.push(name);
    }
    if names.is_empty() {
        return Err(he(format!("{} contains no members", path.display())));
    }
    Ok(names)
}

/// A `.npz` member must be a plain array name, never a path.
///
/// Checked at **both** doors. `npz_members` validates what it lists, but a member name can also arrive
/// from a declarative spec (`format = "npz-member"`, `member = "..."`), and that path went straight to
/// `by_name` — so the listing check alone left the spec able to name `../evil.npy`. A name carrying a
/// path is either an archive that is not a `np.savez` product or a traversal attempt; either way the
/// product name would be nonsense, and `..` must never reach a path join.
#[cfg(feature = "npz")]
fn check_member_name(path: &Path, name: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name.split('/').any(|c| c == "..") {
        return Err(he(format!(
            "{}: member '{name}' is a path, not a plain array name — .npz members written by \
             `np.savez` never are\n  \
             preserve the archive as-is instead:  tessera ingest blob <FILE>",
            path.display()
        )));
    }
    Ok(())
}

/// Read one named member of a `.npz` archive.
#[cfg(feature = "npz")]
pub fn read_npz_member(path: &Path, member: &str) -> Result<NpyContent> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| {
        he(format!(
            "{} is not a readable .npz archive: {e}",
            path.display()
        ))
    })?;
    check_member_name(path, member)?;
    let mut entry = zip.by_name(member).map_err(|e| {
        he(format!(
            "{} has no member '{member}': {e}\n  members: {}",
            path.display(),
            npz_members(path)
                .map(|m| m.join(" · "))
                .unwrap_or_else(|_| "<unreadable>".into())
        ))
    })?;
    // The zip header's declared uncompressed size is attacker-controlled, so it is used ONLY as a
    // cheap early reject and as a *bounded* allocation hint — never as the read limit.
    let declared = entry.size();
    if declared > MAX_NPZ_MEMBER_BYTES {
        return Err(he(format!(
            "{}: member '{member}' declares {declared} bytes, over the {MAX_NPZ_MEMBER_BYTES}-byte \
             limit for a decompressed .npz member\n  \
             preserve it as-is instead:  tessera ingest blob <FILE>",
            path.display()
        )));
    }
    // Capped, not just bounded by the member limit: `Vec::with_capacity` of a *declared* 4 GiB aborts
    // the process on a 200-byte archive, so the hint is only ever an optimisation for the common case.
    // Beyond it the vector grows as the read actually delivers bytes.
    const HINT_CAP: u64 = 64 << 20; // 64 MiB
    let hint = usize::try_from(declared.min(MAX_NPZ_MEMBER_BYTES).min(HINT_CAP)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(hint);
    // `take` is the actual defence: a member that lies *downwards* about its size still cannot read
    // more than the cap, and reading cap+1 is what proves it lied.
    let mut limited = std::io::Read::take(&mut entry, MAX_NPZ_MEMBER_BYTES + 1);
    std::io::Read::read_to_end(&mut limited, &mut bytes)
        .map_err(|e| he(format!("{}: read member '{member}': {e}", path.display())))?;
    if bytes.len() as u64 > MAX_NPZ_MEMBER_BYTES {
        return Err(he(format!(
            "{}: member '{member}' decompressed past the {MAX_NPZ_MEMBER_BYTES}-byte limit while \
             declaring only {declared} — treating it as a decompression bomb\n  \
             preserve it as-is instead:  tessera ingest blob <FILE>",
            path.display()
        )));
    }
    // A member whose actual content disagrees with its declared size is malformed, and saying so beats
    // letting the .npy length check report a confusing truncation further in.
    if bytes.len() as u64 != declared {
        return Err(he(format!(
            "{}: member '{member}' declares {declared} bytes but decompressed to {} — the archive is \
             corrupt or the entry lies about its size",
            path.display(),
            bytes.len()
        )));
    }
    read_npy_bytes(&bytes, &format!("{}::{member}", path.display()))
}

/// The product name for an `.npz` member: the member's filename without the `.npy` suffix.
///
/// `np.savez(f, energy=…)` stores `energy.npy`, so the natural product name is `energy` — the keyword
/// the scientist actually used.
pub fn member_product_name(member: &str) -> &str {
    member.strip_suffix(".npy").unwrap_or(member)
}

/// Does this member name look auto-generated by `np.savez` (positional `arr_0`, `arr_1`, …)?
///
/// ADR-0056 §11 asks for a warning on these, and §9's loudness rule says why it is a warning and not an
/// error: `arr_0` is a real name that ingests perfectly well, but it carries **no meaning**, so a
/// collection full of them is findable by nothing. The operator passed positional arguments to
/// `np.savez` where keyword arguments would have named the arrays; saying so once is worth more than
/// refusing.
pub fn is_auto_generated_member_name(member: &str) -> bool {
    member_product_name(member)
        .strip_prefix("arr_")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
#[cfg(feature = "npz")]
mod npz_tests {
    use super::*;

    /// Build a `.npz` in memory: a zip of `.npy` members. STORED, like `np.savez`.
    /// CRC-32 (IEEE), bitwise — a few lines beats a dependency for one test helper.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Build a STORED zip **by hand**, so the tests can produce archives `zip::ZipWriter` refuses to
    /// write: a duplicated member name, and an entry that lies about its uncompressed size.
    ///
    /// That refusal is itself the reason these guards need hand-built input — our own writer cannot
    /// produce such an archive, but a hostile file written by anything else can, and that is exactly
    /// the input the reader has to survive.
    ///
    /// `declared` overrides the uncompressed size recorded in the headers; `None` means tell the truth.
    fn raw_zip(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, declared) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            let csize = data.len() as u32;
            let usize_field = declared.unwrap_or(csize);
            let n = name.as_bytes();
            for part in [
                &0x0403_4b50u32.to_le_bytes()[..], // local file header
                &10u16.to_le_bytes()[..],          // version needed
                &0u16.to_le_bytes()[..],           // flags
                &0u16.to_le_bytes()[..],           // method = stored
                &0u16.to_le_bytes()[..],           // time
                &0u16.to_le_bytes()[..],           // date
                &crc.to_le_bytes()[..],
                &csize.to_le_bytes()[..],
                &usize_field.to_le_bytes()[..],
                &(n.len() as u16).to_le_bytes()[..],
                &0u16.to_le_bytes()[..], // extra len
                n,
            ] {
                out.extend_from_slice(part);
            }
            out.extend_from_slice(data);
            for part in [
                &0x0201_4b50u32.to_le_bytes()[..], // central directory header
                &10u16.to_le_bytes()[..],          // version made by
                &10u16.to_le_bytes()[..],          // version needed
                &0u16.to_le_bytes()[..],           // flags
                &0u16.to_le_bytes()[..],           // method
                &0u16.to_le_bytes()[..],           // time
                &0u16.to_le_bytes()[..],           // date
                &crc.to_le_bytes()[..],
                &csize.to_le_bytes()[..],
                &usize_field.to_le_bytes()[..],
                &(n.len() as u16).to_le_bytes()[..],
                &0u16.to_le_bytes()[..], // extra len
                &0u16.to_le_bytes()[..], // comment len
                &0u16.to_le_bytes()[..], // disk number
                &0u16.to_le_bytes()[..], // internal attrs
                &0u32.to_le_bytes()[..], // external attrs
                &offset.to_le_bytes()[..],
                n,
            ] {
                central.extend_from_slice(part);
            }
        }
        let cd_offset = out.len() as u32;
        let cd_len = central.len() as u32;
        out.extend_from_slice(&central);
        let count = entries.len() as u16;
        for part in [
            &0x0605_4b50u32.to_le_bytes()[..], // EOCD
            &0u16.to_le_bytes()[..],           // disk
            &0u16.to_le_bytes()[..],           // cd start disk
            &count.to_le_bytes()[..],
            &count.to_le_bytes()[..],
            &cd_len.to_le_bytes()[..],
            &cd_offset.to_le_bytes()[..],
            &0u16.to_le_bytes()[..], // comment len
        ] {
            out.extend_from_slice(part);
        }
        out
    }

    fn npz(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in members {
                zip::ZipWriter::start_file(&mut w, *name, opts).unwrap();
                std::io::Write::write_all(&mut w, bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn f64_le(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn write_npy(descr: &str, shape: &[u64], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&[1, 0]);
        let shape_text = if shape.len() == 1 {
            format!("({},)", shape[0])
        } else {
            format!(
                "({})",
                shape
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let header =
            format!("{{'descr': {descr}, 'fortran_order': False, 'shape': {shape_text}, }}\n");
        out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn an_npz_lists_its_members_and_reads_each_one() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bundle.npz");
        std::fs::write(
            &p,
            npz(&[
                (
                    "energy.npy",
                    write_npy("'<f8'", &[3], &f64_le(&[1.0, 2.0, 3.0])),
                ),
                (
                    "counts.npy",
                    write_npy("'<i4'", &[2], &[1, 0, 0, 0, 2, 0, 0, 0]),
                ),
            ]),
        )
        .unwrap();

        assert_eq!(npz_members(&p).unwrap(), vec!["energy.npy", "counts.npy"]);
        // Member order is the ARCHIVE's order, which is what the expanded spec records — so a
        // collection's member order is reproducible from the file rather than from a directory walk.

        let NpyContent::Array(a) = read_npz_member(&p, "energy.npy").unwrap() else {
            panic!("expected an array")
        };
        assert_eq!(a.spec.shape, vec![3]);
        assert_eq!(a.data, ArrayData::F64(vec![1.0, 2.0, 3.0]));

        let NpyContent::Array(a) = read_npz_member(&p, "counts.npy").unwrap() else {
            panic!("expected an array")
        };
        assert_eq!(a.data, ArrayData::I32(vec![1, 2]));

        // A missing member lists what is there rather than just failing.
        let err = read_npz_member(&p, "nope.npy").unwrap_err().to_string();
        assert!(err.contains("has no member 'nope.npy'"), "got {err}");
        assert!(err.contains("energy.npy"), "lists the real members: {err}");
    }

    #[test]
    fn a_member_name_becomes_the_product_name() {
        assert_eq!(member_product_name("energy.npy"), "energy");
        assert_eq!(member_product_name("no_suffix"), "no_suffix");
    }

    /// ADR-0056 §11's advisory: `arr_0` ingests fine but means nothing, so a collection of them is
    /// findable by nothing. A warning, never an error — the operator used positional `np.savez`
    /// arguments where keywords would have named the arrays.
    #[test]
    fn auto_generated_member_names_are_recognised_without_being_refused() {
        for auto in ["arr_0.npy", "arr_1.npy", "arr_42.npy", "arr_0"] {
            assert!(
                is_auto_generated_member_name(auto),
                "'{auto}' is auto-generated"
            );
        }
        for named in [
            "energy.npy",
            "arr.npy",
            "arr_.npy",
            "arr_x.npy",
            "arrival.npy",
        ] {
            assert!(
                !is_auto_generated_member_name(named),
                "'{named}' is a real name and must not be flagged"
            );
        }
    }

    /// **#461 blocker 3.** `read_npz_member` sized its buffer from `entry.size()` and then
    /// `read_to_end`-ed with no ceiling. Both halves trust the archive: the declared uncompressed size
    /// is attacker-controlled, so a member claiming hundreds of GiB is a huge allocation before a byte
    /// is read, and an unbounded read turns a few KiB of deflate into an OOM-kill. An untrusted file
    /// must not be able to decide how much memory we commit.
    ///
    /// A real bomb is not constructible in a unit test without shipping one, so this drives the guard
    /// from the other side — the declared size — plus the deflate ratio that makes bombs possible.
    #[test]
    fn an_npz_member_cannot_decide_how_much_memory_we_commit() {
        let dir = tempfile::tempdir().unwrap();

        // A member whose DECLARED size is over the cap is rejected before allocating.
        let lying = dir.path().join("lying.npz");
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip::ZipWriter::start_file(&mut w, "big.npy", opts).unwrap();
            // Highly compressible: 8 MiB of zeroes is a few KiB on disk. Not over the cap, but it is
            // the shape a bomb takes, and it proves the read path survives a large ratio.
            std::io::Write::write_all(&mut w, &vec![0u8; 8 << 20]).unwrap();
            w.finish().unwrap();
        }
        std::fs::write(&lying, buf.into_inner()).unwrap();
        // It is not a valid .npy, so it fails — but on the NPY magic, having been read safely, NOT on
        // an allocation failure or a kill.
        let err = read_npz_member(&lying, "big.npy").unwrap_err().to_string();
        assert!(
            err.contains("NUMPY magic") || err.contains("not a NumPy"),
            "a highly compressible member must still be read within the cap, got: {err}"
        );

        // A member that is genuinely fine still reads.
        let ok = dir.path().join("ok.npz");
        std::fs::write(
            &ok,
            npz(&[("x.npy", write_npy("'<f8'", &[2], &f64_le(&[1.0, 2.0])))]),
        )
        .unwrap();
        assert!(read_npz_member(&ok, "x.npy").is_ok());
    }

    /// **#461 blocker 3, the naming half.** `by_name` resolves a duplicate member to one entry
    /// silently, so which array got sealed would depend on zip internals; and a member whose name
    /// carries a path is either not a `np.savez` product or a traversal attempt. Both are refused
    /// rather than guessed at.
    #[test]
    fn ambiguous_or_path_bearing_npz_member_names_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let payload = write_npy("'<f8'", &[1], &f64_le(&[1.0]));

        let nested = dir.path().join("nested.npz");
        std::fs::write(&nested, npz(&[("sub/x.npy", payload.clone())])).unwrap();
        let err = npz_members(&nested).unwrap_err().to_string();
        assert!(err.contains("is a path"), "got: {err}");

        let traversal = dir.path().join("traversal.npz");
        std::fs::write(&traversal, npz(&[("../x.npy", payload.clone())])).unwrap();
        assert!(npz_members(&traversal).is_err());

        // A single ordinary member is still fine — the guards must not reject the normal case.
        let fine = dir.path().join("fine.npz");
        std::fs::write(&fine, npz(&[("only.npy", payload)])).unwrap();
        assert_eq!(npz_members(&fine).unwrap(), vec!["only.npy".to_string()]);
    }

    /// The `.tsra` STORED invariant, tested **here** rather than in `tessera-io`.
    ///
    /// `tessera-io`'s container reader now rejects any entry that is not STORED, because the format's
    /// range-readability depends on it: a block's bytes are fetchable by range only if they are stored
    /// verbatim, which is what lets a cloud reader pull one block out of a multi-GB archive. A deflated
    /// entry would still **verify** — the bytes decompress to the sealed content — so nothing but the
    /// cloud reader would ever notice.
    ///
    /// It lives in this crate because this is the one that can *build* the counter-example. The
    /// workspace pins `zip = { default-features = false }`, so `CompressionMethod::Deflated` does not
    /// even exist in a `-p tessera-io` build; it appears only once **this** crate's `npy` feature pulls
    /// `deflate-flate2` in and cargo unifies features across the graph. That asymmetry is the whole
    /// hazard: the invariant used to be enforced by the absence of a decompressor, and adding one for
    /// `.npz` would have quietly downgraded it to a comment.
    #[test]
    fn a_deflated_tsra_entry_is_refused_by_the_container_reader() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("deflated.tsra");

        // A STORED mimetype (so it is recognisably a .tsra) plus one DEFLATED entry. No sealed manifest
        // is needed: the invariant is structural, so the reader checks it before interpreting content.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let stored: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip::ZipWriter::start_file(&mut w, "mimetype", stored).unwrap();
            std::io::Write::write_all(&mut w, tessera_io::container::MIMETYPE.as_bytes()).unwrap();
            let deflated: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip::ZipWriter::start_file(&mut w, "blocks/data", deflated).unwrap();
            std::io::Write::write_all(&mut w, &vec![0u8; 4096]).unwrap();
            w.finish().unwrap();
        }
        std::fs::write(&p, buf.into_inner()).unwrap();

        let err = match tessera_io::container::Reader::open(&p) {
            Ok(_) => panic!("a deflated .tsra entry must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("STORED"),
            "the error should name the invariant, got: {err}"
        );
    }

    /// **The guard has to sit in front of the thing it guards.** An earlier revision ran the STORED
    /// scan *after* reading `mimetype`, so the friendlier "not a .tsra" error survived for a random
    /// zip — but that read is unbounded, and `mimetype` is itself an entry. A `.tsra` whose mimetype
    /// member is a deflate bomb therefore exhausted memory *before* the check that would have refused
    /// it.
    ///
    /// This is the bomb-shaped case: ~100 MiB of zeroes in the mimetype member, a few KiB on disk. It
    /// must be refused from the central directory alone, without the payload ever being inflated.
    #[test]
    fn a_deflate_bomb_in_the_mimetype_member_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bomb.tsra");
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let deflated: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip::ZipWriter::start_file(&mut w, "mimetype", deflated).unwrap();
            std::io::Write::write_all(&mut w, &vec![0u8; 100 << 20]).unwrap();
            w.finish().unwrap();
        }
        let bytes = buf.into_inner();
        // Compressible enough that the on-disk size is nothing like the decompressed size — which is
        // the whole point: the reader must never be the one to find that out.
        assert!(
            bytes.len() < 1 << 20,
            "the fixture should be small on disk, is {} bytes",
            bytes.len()
        );
        std::fs::write(&p, &bytes).unwrap();

        let err = match tessera_io::container::Reader::open(&p) {
            Ok(_) => panic!("a deflate-bombed mimetype must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("STORED"),
            "it must be refused for the STORED invariant, from the central directory, rather than \
             after inflating 100 MiB: {err}"
        );
    }

    /// **#461 blocker 3, the ambiguity half — and the guard we deliberately do NOT have.**
    ///
    /// A duplicated member name would be ambiguous: `by_name` resolves it to one entry, so *which*
    /// array got sealed could depend on zip internals. `zip::ZipWriter` refuses to write such an
    /// archive, so this one is hand-built — and the finding is that `ZipArchive` **already collapses**
    /// duplicates, reporting a single member. There is therefore nothing left to refuse, and a check
    /// in `npz_members` would be unreachable code dressed up as a defence.
    ///
    /// What matters for a content-addressed format is that the resolution is *deterministic*, so that
    /// is what this pins — along with the upstream behaviour itself, so a `zip` bump that started
    /// surfacing both entries fails here rather than silently making the ingest ambiguous.
    #[test]
    fn a_duplicated_npz_member_name_collapses_in_the_zip_reader() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_npy("'<f8'", &[1], &f64_le(&[1.0]));
        let second = write_npy("'<f8'", &[1], &f64_le(&[2.0]));
        let p = dir.path().join("dup.npz");
        std::fs::write(
            &p,
            raw_zip(&[("x.npy", &first, None), ("x.npy", &second, None)]),
        )
        .unwrap();

        assert_eq!(
            npz_members(&p).expect("the reader collapses duplicates"),
            vec!["x.npy".to_string()],
            "a `zip` bump that surfaced both entries would make this ingest ambiguous"
        );
        // Deterministic: the same archive resolves to the same values every time, so whichever entry
        // the reader keeps, the sealed bytes are a function of the file and not of the run.
        let NpyContent::Array(a) = read_npz_member(&p, "x.npy").expect("reads") else {
            panic!("expected an array");
        };
        let NpyContent::Array(b) = read_npz_member(&p, "x.npy").expect("reads") else {
            panic!("expected an array");
        };
        assert_eq!(a.data, b.data);
    }

    /// **#461 blocker 3, the lying-size half.** The old reader took `entry.size()` — the *declared*
    /// uncompressed size, which the archive controls — as its allocation size. An entry that lies is
    /// therefore the shape of the attack, and the reader must notice the disagreement rather than hand
    /// a short buffer to the `.npy` parser and report a confusing truncation.
    #[test]
    fn an_npz_entry_that_lies_about_its_size_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let payload = write_npy("'<f8'", &[1], &f64_le(&[1.0]));
        let p = dir.path().join("liar.npz");
        std::fs::write(
            &p,
            raw_zip(&[("x.npy", &payload, Some(payload.len() as u32 + 4096))]),
        )
        .unwrap();
        let err = read_npz_member(&p, "x.npy").unwrap_err().to_string();
        assert!(
            err.contains("declares") && err.contains("decompressed to"),
            "the mismatch should be named, got: {err}"
        );
    }

    #[test]
    fn an_empty_or_unreadable_archive_is_refused_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.npz");
        std::fs::write(&empty, npz(&[])).unwrap();
        let err = npz_members(&empty).unwrap_err().to_string();
        assert!(err.contains("contains no members"), "got {err}");

        let bogus = dir.path().join("bogus.npz");
        std::fs::write(&bogus, b"not a zip at all").unwrap();
        let err = npz_members(&bogus).unwrap_err().to_string();
        assert!(err.contains("not a readable .npz archive"), "got {err}");
        assert!(
            err.contains("--from npy"),
            "offers the single-array route: {err}"
        );
    }

    /// `np.savez_compressed` writes DEFLATE members; they must read too, since it is the commoner call
    /// for anything large.
    #[test]
    fn a_deflate_compressed_member_reads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("z.npz");
        let member = write_npy("'<f8'", &[3], &f64_le(&[1.0, 2.0, 3.0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip::ZipWriter::start_file(&mut w, "energy.npy", opts).unwrap();
            std::io::Write::write_all(&mut w, &member).unwrap();
            w.finish().unwrap();
        }
        std::fs::write(&p, buf.into_inner()).unwrap();
        let NpyContent::Array(a) = read_npz_member(&p, "energy.npy").unwrap() else {
            panic!("expected an array")
        };
        assert_eq!(a.data, ArrayData::F64(vec![1.0, 2.0, 3.0]));
    }
}
