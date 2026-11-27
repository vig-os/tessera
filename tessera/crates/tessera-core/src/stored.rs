//! The stored layout of a sealed JSON document (ADR-0066 rules 1–5 and 10): the names of the
//! document and its seal member, the seal member's format, and the refusal of the alpha layout.
//!
//! A sealed document is two members side by side:
//!
//! - the document (`manifest.json`, `collection.json`), stored as exactly its RFC 8785 (JCS)
//!   bytes, with no `manifest_hash` key;
//! - its seal (`manifest.seal`, `collection.seal`): the 71 ASCII bytes
//!   `"blake3:" + hex(blake3(document bytes))`, nothing else (no trailing newline).
//!
//! In a `.tsra` the seal is zip entry 2, after `mimetype` and before `manifest.json`, so one ranged
//! read gets both; in the exploded form and beside a collection it is a file next to the document.
//! A sub-collection member stored as `<stem>.collection.json` has its seal at
//! `<stem>.collection.seal` ([`seal_name_for`]).
//!
//! Every reader goes through [`crate::Manifest::from_stored_members`] /
//! [`crate::Collection::from_stored_members`], which take the seal member as found (or `None`): a
//! missing seal member is the alpha layout, refused with [`crate::Error::UnsupportedLayout`] naming
//! what was found and what is required; a present one must be well formed
//! ([`crate::Error::MalformedSeal`]) and is then checked by `from_stored`.

use serde_json::Value;

/// The product document's member name.
pub const MANIFEST_MEMBER: &str = "manifest.json";
/// The product seal's member name (ADR-0066 rule 3).
pub const MANIFEST_SEAL_MEMBER: &str = "manifest.seal";
/// The collection document's file name.
pub const COLLECTION_FILE: &str = "collection.json";
/// The collection seal's file name (ADR-0066 rule 5).
pub const COLLECTION_SEAL_FILE: &str = "collection.seal";
/// The length of a seal member: `blake3:` plus 64 hex digits.
pub const SEAL_LEN: usize = 71;

/// The seal member's name for a document stored as `name`: `.json` becomes `.seal`
/// (`manifest.json` → `manifest.seal`, `<stem>.collection.json` → `<stem>.collection.seal`).
/// `None` when `name` does not end in `.json`.
pub fn seal_name_for(name: &str) -> Option<String> {
    name.strip_suffix(".json")
        .map(|stem| format!("{stem}.seal"))
}

/// The seal a seal member holds, checked for its format: exactly [`SEAL_LEN`] bytes,
/// `blake3:` and 64 lower-case hex digits. The value is compared with the document's digest by
/// `from_stored`; this only refuses what can't be a seal at all.
///
/// # Errors
/// [`crate::Error::MalformedSeal`] naming `member` and the content found.
pub fn parse_seal<'a>(bytes: &'a [u8], member: &str) -> crate::Result<&'a str> {
    let well_formed = bytes.len() == SEAL_LEN
        && bytes.starts_with(b"blake3:")
        && bytes[7..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
    match std::str::from_utf8(bytes) {
        Ok(s) if well_formed => Ok(s),
        _ => Err(crate::Error::MalformedSeal {
            member: member.to_owned(),
            found: bytes[..bytes.len().min(80)].escape_ascii().to_string(),
        }),
    }
}

/// The refusal of a document stored as `document` and found without its seal member `seal`
/// (ADR-0066 rule 10): the evidence of what was found, in words, and the layout required. Reads the
/// bytes only to describe them. The document is called the pre-ADR-0066 (alpha) layout only when
/// its bytes show it (an in-band `manifest_hash`, or bytes that are not their JCS form); JCS bytes
/// without the key are an ADR-0066 document whose seal member is missing.
pub(crate) fn unsupported_layout(document: &str, seal: &str, bytes: &[u8]) -> crate::Error {
    let mut found = Vec::new();
    let mut alpha = false;
    match crate::canonical::parse_strict(&String::from_utf8_lossy(bytes)) {
        Ok(v) => {
            match v.get("tessera_version") {
                Some(Value::String(ver)) => {
                    found.push(format!("`{document}` declares tessera_version {ver:?}"))
                }
                _ => found.push(format!("`{document}` declares no tessera_version")),
            }
            found.push(format!("there is no `{seal}`"));
            if v.get(crate::manifest::SEAL_KEY).is_some() {
                alpha = true;
                found.push(format!(
                    "`{document}` carries an in-band `manifest_hash` key"
                ));
            }
            if crate::canonical::check_canonical(bytes).is_err() {
                alpha = true;
                found.push(format!(
                    "`{document}` is not stored as its JCS bytes (a pretty or respelled layout)"
                ));
            }
        }
        Err(e) => {
            found.push(format!("there is no `{seal}`"));
            found.push(format!("`{document}` does not parse as I-JSON ({e})"));
        }
    }
    let kind = if alpha {
        "a pre-ADR-0066 (alpha) layout"
    } else {
        "a document without its seal member"
    };
    crate::Error::UnsupportedLayout {
        found: format!("{kind}: {}", found.join(", ")),
        required: format!(
            "`{document}` stored as exactly its RFC 8785 JCS bytes with no `manifest_hash` key, \
             and its seal `blake3:<64 hex>` = blake3 of those bytes in `{seal}`"
        ),
    }
}

/// The seal member of the document stored as `document`, as found, or the refusal when it is
/// missing; then its format.
///
/// # Errors
/// [`crate::Error::Invalid`] when `document` does not end in `.json` (it has no seal name);
/// [`crate::Error::UnsupportedLayout`] without a seal; [`crate::Error::MalformedSeal`].
pub(crate) fn require_seal<'a>(
    document: &str,
    document_bytes: &[u8],
    seal: Option<&'a [u8]>,
) -> crate::Result<&'a str> {
    let seal_name = seal_name_for(document).ok_or_else(|| {
        crate::Error::Invalid(format!(
            "`{document}`: a sealed document's name ends in `.json`"
        ))
    })?;
    match seal {
        None => Err(unsupported_layout(document, &seal_name, document_bytes)),
        Some(s) => parse_seal(s, &seal_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_names_follow_the_document_name() {
        assert_eq!(seal_name_for("manifest.json").unwrap(), "manifest.seal");
        assert_eq!(
            seal_name_for("blake3%3aab.collection.json").unwrap(),
            "blake3%3aab.collection.seal"
        );
        assert_eq!(seal_name_for("manifest"), None);
    }

    #[test]
    fn a_seal_member_is_exactly_71_ascii_bytes_of_lower_case_hex() {
        let ok = format!("blake3:{}", "0a".repeat(32));
        assert_eq!(parse_seal(ok.as_bytes(), "manifest.seal").unwrap(), ok);
        for bad in [
            format!("{ok}\n"),
            ok.to_uppercase(),
            // Only the hex upper-cased, prefix intact (#939 item 1).
            format!("blake3:{}", "0A".repeat(32)),
            ok.replace("blake3:", "sha256:"),
            ok[..70].to_string(),
            format!("blake3:{}", "0g".repeat(32)),
            String::new(),
        ] {
            let e = parse_seal(bad.as_bytes(), "manifest.seal").unwrap_err();
            assert!(
                matches!(
                    e,
                    crate::Error::MalformedSeal { ref member, .. } if member == "manifest.seal"
                ),
                "{bad:?}: {e:?}"
            );
        }
    }

    #[test]
    fn the_alpha_layout_refusal_names_what_it_found_and_what_it_requires() {
        let alpha = "{\n  \"tessera_version\": \"0.0.0\",\n  \"manifest_hash\": \"blake3:00\"\n}";
        let e = require_seal(MANIFEST_MEMBER, alpha.as_bytes(), None).unwrap_err();
        let msg = e.to_string();
        for needle in [
            "tessera_version \"0.0.0\"",
            "there is no `manifest.seal`",
            "in-band `manifest_hash`",
            "not stored as its JCS bytes",
            "requires the ADR-0066 layout",
            "no migration",
            "(alpha) layout",
        ] {
            assert!(msg.contains(needle), "{needle:?} missing from: {msg}");
        }
    }

    /// #939 items 2 and 3: JCS bytes without the in-band key and without their seal member are not
    /// called the alpha layout nor "not stored as its JCS bytes", and a sub-collection member's
    /// refusal names its own seal file.
    #[test]
    fn a_jcs_document_without_its_seal_is_not_called_the_alpha_layout() {
        let jcs = br#"{"tessera_version":"0.0.0"}"#;
        for (document, seal) in [
            (MANIFEST_MEMBER, "manifest.seal"),
            ("blake3%3aab.collection.json", "blake3%3aab.collection.seal"),
        ] {
            let msg = require_seal(document, jcs, None).unwrap_err().to_string();
            assert!(msg.contains(&format!("there is no `{seal}`")), "{msg}");
            assert!(msg.contains(&format!("in `{seal}`")), "{msg}");
            assert!(msg.contains("a document without its seal member"), "{msg}");
            for absent in ["(alpha) layout", "not stored as its JCS bytes", "in-band"] {
                assert!(!msg.contains(absent), "{absent:?} in: {msg}");
            }
        }
        let e = require_seal("blake3%3aab.collection.json", jcs, Some(b"x")).unwrap_err();
        assert!(
            matches!(&e, crate::Error::MalformedSeal { member, .. } if member == "blake3%3aab.collection.seal"),
            "{e:?}"
        );
        assert!(matches!(
            require_seal("collection", jcs, None),
            Err(crate::Error::Invalid(_))
        ));
    }
}
