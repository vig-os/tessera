//! A path to a key or element inside a parsed JSON object, as structured segments (#873).
//!
//! The newer-field walk (ADR-0060 §2.2) and the spelling check
//! ([`crate::canonical::from_declared_form`]) agree on *where* a key is by comparing these paths
//! segment by segment, never by a string spelling of them. A string spelling is not injective: a
//! top-level key literally named `members[0].kind` and the `kind` of the first member both spell
//! `members[0].kind` in dotted form, which once let a crafted newer key exempt a declared key from
//! the spelling check.
//!
//! Paths are rendered for people (warnings, [`crate::Error::NewerFormatFields`],
//! [`crate::Manifest::newer_fields`], [`crate::Collection::newer_fields`],
//! [`crate::ProductSchema::view_of`]) by the one [`Display`](std::fmt::Display) impl here, which is
//! injective, so two different paths never read the same:
//!
//! - a key made only of ASCII letters, digits and `_`, not starting with a digit, is written bare,
//!   after a `.` unless it is the first segment: `producer.tool`;
//! - any other key is written as its JSON string literal in brackets: `["members[0].kind"]`,
//!   `tables["events.v2"].order`, `[""]`, `["0"]`;
//! - an array index is written in brackets as a decimal number: `members[0]`.
//!
//! The rendering parses back in exactly one way: a bare key holds no `.`, `[` or `"`, and after a
//! `[` a `"` opens a JSON string (which ends at its first unescaped `"`) while a digit opens an
//! index. So every path a well-formed format declares (`blocks[0].stats`, `level.uri`,
//! `tables.events.row_count_min`) reads exactly as its dotted form always did.

use std::fmt;

/// One step of a [`FieldPath`]: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Key(String),
    Index(usize),
}

/// A path from the root of a parsed JSON value to one key or element (see the module docs). The
/// empty path is the value itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldPath(Vec<Segment>);

impl FieldPath {
    /// The path to the value itself.
    pub(crate) fn root() -> Self {
        Self::default()
    }

    /// Whether this is the path to the value itself.
    pub(crate) fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// This path extended by the object key `key`.
    pub(crate) fn key(&self, key: &str) -> Self {
        self.with(Segment::Key(key.to_owned()))
    }

    /// This path extended by the array index `i`.
    pub(crate) fn index(&self, i: usize) -> Self {
        self.with(Segment::Index(i))
    }

    /// `rest`, read from this path instead of from the root (`schema` + `fields[0].pattern` is
    /// `schema.fields[0].pattern`).
    pub(crate) fn join(&self, rest: &FieldPath) -> Self {
        let mut out = self.clone();
        out.0.extend(rest.0.iter().cloned());
        out
    }

    fn with(&self, segment: Segment) -> Self {
        let mut out = self.clone();
        out.0.push(segment);
        out
    }
}

/// Every path in `paths`, rendered (the one rendering every report of newer fields uses).
pub(crate) fn render(paths: &[FieldPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

/// Whether `key` is written bare: ASCII letters, digits and `_`, not empty, not starting with a digit.
fn is_bare(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            match segment {
                Segment::Key(k) if is_bare(k) => {
                    if i > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(k)?;
                }
                // A JSON string literal: `Value`'s Display is its compact JSON text.
                Segment::Key(k) => write!(f, "[{}]", serde_json::Value::from(k.as_str()))?,
                Segment::Index(n) => write!(f, "[{n}]")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldPath, Segment};
    use std::collections::HashMap;

    fn path(segments: &[Segment]) -> FieldPath {
        FieldPath(segments.to_vec())
    }

    #[test]
    fn declared_paths_render_in_their_dotted_form() {
        let root = FieldPath::root();
        assert_eq!(root.to_string(), "");
        assert_eq!(
            root.key("producer").key("tool").to_string(),
            "producer.tool"
        );
        assert_eq!(
            root.key("members").index(0).key("kind").to_string(),
            "members[0].kind"
        );
        assert_eq!(
            root.key("schema")
                .join(&root.key("tables").key("events").key("row_count_min"))
                .to_string(),
            "schema.tables.events.row_count_min"
        );
    }

    #[test]
    fn any_other_key_is_quoted() {
        let root = FieldPath::root();
        assert_eq!(
            root.key("members[0].kind").to_string(),
            r#"["members[0].kind"]"#
        );
        assert_eq!(
            root.key("tables").key("events.v2").key("order").to_string(),
            r#"tables["events.v2"].order"#
        );
        assert_eq!(root.key("").to_string(), r#"[""]"#);
        assert_eq!(root.key("0").to_string(), r#"["0"]"#);
        assert_eq!(root.key("a\"b").to_string(), r#"["a\"b"]"#);
    }

    /// Every path of up to three segments over keys built to collide (`.`, `[0]`, `/`, `~`, `"`, an
    /// escape, an empty key, a key spelling an index) renders differently from every other one.
    #[test]
    fn rendering_is_injective() {
        let keys = [
            "a", "b", "a.b", "a[0]", "[0]", "0", "", ".", "/", "~", "~0", "~1", "a/b", "\"",
            "a\"]", "\"]", "\\", "a b", "é", "_", "a.b.c", "[\"a\"]",
        ];
        let mut segments: Vec<Segment> = keys.iter().map(|k| Segment::Key((*k).into())).collect();
        segments.extend([0, 1, 10].map(Segment::Index));
        let mut paths = vec![Vec::new()];
        let mut frontier = vec![Vec::new()];
        for _ in 0..3 {
            let next: Vec<Vec<Segment>> = frontier
                .iter()
                .flat_map(|p: &Vec<Segment>| {
                    segments.iter().map(move |s| {
                        let mut p = p.clone();
                        p.push(s.clone());
                        p
                    })
                })
                .collect();
            paths.extend(next.iter().cloned());
            frontier = next;
        }
        let mut seen: HashMap<String, Vec<Segment>> = HashMap::new();
        for p in &paths {
            if let Some(other) = seen.insert(path(p).to_string(), p.clone()) {
                panic!("{other:?} and {p:?} both render as {}", path(p));
            }
        }
        assert_eq!(seen.len(), paths.len());
    }
}
