//! Block names — the one validated type every block name in the format passes through (ADR-0063).
//!
//! A block name is a **relative path of one or more `/`-separated segments**:
//!
//! ```text
//! name    = segment *( "/" segment )        ; at most 255 bytes in total
//! segment = 1*( ALPHA / DIGIT / "." / "_" / "-" )   ; but not "." and not ".."
//! ```
//!
//! The name is the key of a sealed `.tsra`'s `blocks/<name>` zip member, of an exploded directory's
//! `blocks/<name>` file, of a write session's fragment, and of an object-store prefix. Because a
//! name is written under a base directory in all of those places, the grammar is also the security
//! boundary of #685: a name like `../x` or `/etc/x` would otherwise escape the base directory.
//!
//! **Why the grammar is complete against escaping its base.** A segment is non-empty and built only
//! from `[A-Za-z0-9._-]`, so it contains no separator on any platform (`/`, `\`), no drive or
//! verbatim prefix (those need `:` or `\`), no NUL, and is neither `.` nor `..`. Each segment is
//! therefore exactly one [`Component::Normal`] on every platform `std` supports, and a path built
//! by pushing only `Normal` components onto a base stays under it. [`BlockName::path_under`]
//! re-checks this at the filesystem boundary instead of relying on the grammar alone.
//!
//! **The leaf rule.** Within one product no block name may be a parent prefix of another
//! (`volume` and `volume/1` cannot both exist), so zip members, exploded directories and
//! object-store prefixes all mirror the names one-to-one: a name is either a file or a directory,
//! never both. [`check_leaf_rule`] enforces it; sealing and every writer apply it. Readers do not:
//! a product sealed before the rule (an older `tessera pyramid` output, `volume` + `volume/1`)
//! still opens, verifies and reads in-zip, but cannot be exploded to a directory.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The longest block name, in bytes (a common filename limit; the whole name, not a segment).
pub const MAX_BLOCK_NAME_BYTES: usize = 255;

/// Why a string is not a valid block name (or relative member path) — see the module grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NameError {
    /// The name is the empty string.
    #[error("the name is empty")]
    Empty,
    /// The name is longer than [`MAX_BLOCK_NAME_BYTES`].
    #[error("the name is {len} bytes long, over the {MAX_BLOCK_NAME_BYTES}-byte limit")]
    TooLong { len: usize },
    /// A segment is empty: the name starts or ends with `/` (an absolute path, a trailing slash) or
    /// contains `//`.
    #[error("it has an empty segment (a leading or trailing '/', or '//')")]
    EmptySegment,
    /// A segment is `.` or `..`.
    #[error("segment '{0}' is a relative-path reference, not a name")]
    DotSegment(String),
    /// A character outside `[A-Za-z0-9._-]` (and not the `/` separator): a `\`, a `:` (a Windows
    /// drive), a NUL, a space, any non-ASCII character, …
    #[error("character {0:?} is not allowed (a segment is [A-Za-z0-9._-])")]
    DisallowedChar(char),
    /// Mapping the name onto a directory met a path component that is not a plain name. The grammar
    /// rules this out on every platform `std` supports; this is the filesystem boundary's own check.
    #[error("it does not map onto plain path components")]
    NotRelative,
}

/// Check `s` against the block-name grammar (the module docs). The leaf rule is a property of a
/// *set* of names and is checked by [`check_leaf_rule`].
pub fn check_name(s: &str) -> Result<(), NameError> {
    if s.is_empty() {
        return Err(NameError::Empty);
    }
    if s.len() > MAX_BLOCK_NAME_BYTES {
        return Err(NameError::TooLong { len: s.len() });
    }
    for segment in s.split('/') {
        if segment.is_empty() {
            return Err(NameError::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return Err(NameError::DotSegment(segment.to_string()));
        }
        if let Some(c) = segment
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(NameError::DisallowedChar(c));
        }
    }
    Ok(())
}

/// Join a relative member path that obeys the block-name grammar onto `base`, by plain
/// ([`Component::Normal`]) components only — the one way a name from a manifest, a journal or a
/// zip archive becomes a filesystem path. Used for block names ([`BlockName::path_under`]) and for
/// the other names a `.tsra` carries under a base directory (its `aux/` members).
pub fn join_checked(base: &Path, rel: &str) -> Result<PathBuf, NameError> {
    check_name(rel)?;
    let mut out = base.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(c) => out.push(c),
            _ => return Err(NameError::NotRelative),
        }
    }
    Ok(out)
}

/// A validated block name: the grammar in the module docs holds for every value of this type.
///
/// Construct one with [`BlockName::new`] (or `str::parse` / `TryFrom`), which refuses an invalid
/// name with [`crate::Error::InvalidBlockName`]. Deserializing (a manifest, a journal line)
/// applies the same check, so an invalid name can't enter through a file either. It serializes as
/// the plain string, so the manifest bytes are unchanged by the type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockName(String);

impl BlockName {
    /// Validate `name` against the grammar.
    pub fn new(name: impl Into<String>) -> crate::Result<Self> {
        let name = name.into();
        match check_name(&name) {
            Ok(()) => Ok(BlockName(name)),
            Err(reason) => Err(crate::Error::InvalidBlockName { name, reason }),
        }
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name's `/`-separated segments.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Whether `self` is a proper parent prefix of `other` (`volume` of `volume/1`, not of
    /// `volume1` and not of itself): the relation the leaf rule forbids within one product.
    pub fn is_parent_of(&self, other: &BlockName) -> bool {
        other
            .0
            .strip_prefix(self.0.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    }

    /// The path of this name under `base`, built from plain path components only (see
    /// [`join_checked`]). Every filesystem path derived from a block name goes through here, so no
    /// name can reach outside `base`. It never fails for a value of this type; the check is kept at
    /// the boundary rather than trusted to the grammar ([`crate::Error::InvalidBlockName`] if it did).
    pub fn path_under(&self, base: &Path) -> crate::Result<PathBuf> {
        join_checked(base, &self.0).map_err(|reason| crate::Error::InvalidBlockName {
            name: self.0.clone(),
            reason,
        })
    }

    /// The owned string.
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Enforce the **leaf rule** over the names of one product: no name is a parent prefix of another
/// (module docs). A conflict is [`crate::Error::BlockNameConflict`] naming both. Duplicates are not
/// this rule's concern. Linear in the total length of the names.
pub fn check_leaf_rule<'a>(names: impl IntoIterator<Item = &'a BlockName>) -> crate::Result<()> {
    let names: Vec<&BlockName> = names.into_iter().collect();
    let all: BTreeSet<&str> = names.iter().map(|n| n.as_str()).collect();
    for child in &names {
        let s = child.as_str();
        for (i, _) in s.match_indices('/') {
            let parent = &s[..i];
            if all.contains(parent) {
                return Err(crate::Error::BlockNameConflict {
                    parent: parent.to_string(),
                    child: s.to_string(),
                });
            }
        }
    }
    Ok(())
}

impl fmt::Display for BlockName {
    /// Formats like the string, width and alignment included (`{:<20}` pads a block-name column).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl AsRef<str> for BlockName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for BlockName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for BlockName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for BlockName {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        BlockName::new(s)
    }
}

impl TryFrom<&str> for BlockName {
    type Error = crate::Error;
    fn try_from(s: &str) -> crate::Result<Self> {
        BlockName::new(s)
    }
}

impl TryFrom<String> for BlockName {
    type Error = crate::Error;
    fn try_from(s: String) -> crate::Result<Self> {
        BlockName::new(s)
    }
}

impl From<BlockName> for String {
    fn from(n: BlockName) -> String {
        n.0
    }
}

impl PartialEq<str> for BlockName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for BlockName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for BlockName {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<BlockName> for str {
    fn eq(&self, other: &BlockName) -> bool {
        self == other.0
    }
}

impl PartialEq<BlockName> for &str {
    fn eq(&self, other: &BlockName) -> bool {
        *self == other.0
    }
}

impl PartialEq<BlockName> for String {
    fn eq(&self, other: &BlockName) -> bool {
        *self == other.0
    }
}

impl Serialize for BlockName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlockName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        BlockName::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bad shape #685 names, with the reason the grammar gives.
    pub(crate) const BAD: &[(&str, &str)] = &[
        ("", "empty"),
        (".", "dot"),
        ("..", "dot"),
        ("../x", "dot"),
        ("a/../b", "dot"),
        ("a/.", "dot"),
        ("/abs", "empty segment"),
        ("/", "empty segment"),
        ("trailing/", "empty segment"),
        ("a//b", "empty segment"),
        ("a\\b", "char"),
        ("..\\x", "char"),
        ("nul\0x", "char"),
        ("C:", "char"),
        ("C:\\x", "char"),
        ("c:x", "char"),
        ("has space", "char"),
        ("tab\t", "char"),
        ("é", "char"),
        ("a*b", "char"),
    ];

    #[test]
    fn every_bad_shape_is_refused_with_its_reason() {
        for (bad, why) in BAD {
            let e = check_name(bad).expect_err(bad);
            let ok = match *why {
                "empty" => e == NameError::Empty,
                "dot" => matches!(e, NameError::DotSegment(_)),
                "empty segment" => e == NameError::EmptySegment,
                "char" => matches!(e, NameError::DisallowedChar(_)),
                _ => false,
            };
            assert!(ok, "{bad:?}: {e:?} is not {why}");
            assert!(matches!(
                BlockName::new(*bad),
                Err(crate::Error::InvalidBlockName { .. })
            ));
        }
        let long = "a".repeat(MAX_BLOCK_NAME_BYTES + 1);
        assert_eq!(
            check_name(&long),
            Err(NameError::TooLong {
                len: MAX_BLOCK_NAME_BYTES + 1
            })
        );
        assert!(check_name(&"a".repeat(MAX_BLOCK_NAME_BYTES)).is_ok());
    }

    #[test]
    fn join_checked_refuses_every_bad_shape_without_building_a_path() {
        let base = Path::new("/base/aux");
        for (bad, _) in BAD {
            assert!(join_checked(base, bad).is_err(), "{bad:?}");
        }
        assert!(join_checked(base, &"a".repeat(MAX_BLOCK_NAME_BYTES + 1)).is_err());
    }

    #[test]
    fn valid_names_map_onto_plain_components_under_the_base() {
        let base = Path::new("/base/blocks");
        for good in [
            "volume",
            "volume/0",
            "a/b/c",
            "events_0007",
            "volume.cidx",
            "x..y",
            "...",
            ".hidden",
            "-",
        ] {
            let n = BlockName::new(good).unwrap();
            let p = n.path_under(base).unwrap();
            assert!(p.starts_with(base), "{good}");
            assert_eq!(p.strip_prefix(base).unwrap(), Path::new(good));
            assert!(Path::new(good)
                .components()
                .all(|c| matches!(c, Component::Normal(_))));
        }
    }

    #[test]
    fn serde_round_trips_as_a_plain_string_and_refuses_a_bad_name() {
        let n = BlockName::new("volume/1").unwrap();
        assert_eq!(
            format!("[{n:<10}]"),
            "[volume/1  ]",
            "Display honours width"
        );
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"volume/1\"");
        let back: BlockName = serde_json::from_str("\"volume/1\"").unwrap();
        assert_eq!(back, n);
        let e = serde_json::from_str::<BlockName>("\"../x\"").unwrap_err();
        assert!(e.to_string().contains("invalid block name '../x'"), "{e}");
    }

    #[test]
    fn the_leaf_rule_refuses_a_name_that_is_also_a_parent() {
        let names = |v: &[&str]| {
            v.iter()
                .map(|s| BlockName::new(*s).unwrap())
                .collect::<Vec<_>>()
        };
        // `volume.cidx` sorts between `volume` and `volume/1`; the rule must still see the pair.
        let bad = names(&["volume", "volume.cidx", "volume/1"]);
        match check_leaf_rule(&bad) {
            Err(crate::Error::BlockNameConflict { parent, child }) => {
                assert_eq!((parent.as_str(), child.as_str()), ("volume", "volume/1"));
            }
            other => panic!("{other:?}"),
        }
        assert!(check_leaf_rule(&names(&["a", "a/b/c"])).is_err());
        assert!(
            check_leaf_rule(&names(&["volume/0", "volume/1", "volume1", "volume.cidx"])).is_ok()
        );
        assert!(BlockName::new("a")
            .unwrap()
            .is_parent_of(&BlockName::new("a/b").unwrap()));
        assert!(!BlockName::new("a")
            .unwrap()
            .is_parent_of(&BlockName::new("ab").unwrap()));
        assert!(!BlockName::new("a")
            .unwrap()
            .is_parent_of(&BlockName::new("a").unwrap()));
    }
}
