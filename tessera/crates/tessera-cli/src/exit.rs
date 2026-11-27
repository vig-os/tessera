//! Stable CLI failure classes. Codes 0 (success) and 2 (clap usage) are owned by their callers;
//! this is the only library-error-to-exit-code map. Classification uses the outer error: a damaged
//! member refuses a commit (5), whereas the same damage encountered by GC aborts the store walk (6).
use tessera_core::Error;

/// Keep existing codes 1 and 3; reserve 4–6 for repository policy. The library's exhaustive
/// classifier preserves read/write context; this closed match assigns the process exit numbers.
pub fn code(error: &Error) -> u8 {
    use tessera_core::error::FailureClass::*;
    match error.failure_class() {
        Other => 1,
        BackendUnavailable => 3,
        Conflict => 4,
        RuleRefusal => 5,
        StoreDamaged => 6,
    }
}

/// Logical command target retained for diagnostics. A collection commit/import supplies its sealed
/// id once the file is read.
pub enum Target {
    Pin(String),
    Lineage(String),
    Version(String),
}

/// JSON string syntax makes spaces, quotes, newlines and other ASCII controls unambiguous.
pub fn quote(name: &str) -> String {
    serde_json::to_string(name).expect("serializing a string cannot fail")
}

/// Capture the logical name supplied by the caller before the command consumes its arguments.
pub fn target(cmd: &crate::Cmd) -> Option<Target> {
    use crate::{Cmd, CollectionAction, PinAction};
    match cmd {
        Cmd::Pin {
            action: PinAction::Set { name, .. } | PinAction::Rm { name, .. },
        } => Some(Target::Pin(name.clone())),
        Cmd::Collection {
            action: CollectionAction::Log {
                reference, history, ..
            },
        } => Some(if *history {
            Target::Version(reference.clone())
        } else {
            Target::Lineage(reference.clone())
        }),
        Cmd::Commit { lineage, .. } | Cmd::Log { lineage, .. } | Cmd::Forget { lineage, .. } => {
            Some(Target::Lineage(lineage.clone()))
        }
        _ => None,
    }
}

// Listing has no single caller-supplied name. Decode only canonical layout-2 ref filenames
// for presentation (ADR-0067 D1); verify the inverse with the shared public encoder. No store I/O.
fn ref_target(error: &Error) -> Option<Target> {
    let path = match error {
        Error::RefKindMismatch { path, .. } | Error::RefTargetUnreadable { path, .. } => path,
        _ => return None,
    };
    let namespace = path.parent()?.file_name()?.to_str()?;
    if !matches!(namespace, "pins" | "lineages") {
        return None;
    }
    let encoded = path.file_name()?.to_str()?;
    let mut decoded = Vec::new();
    let mut bytes = encoded.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?
        } else {
            byte
        });
    }
    let name = String::from_utf8(decoded).ok()?;
    if tessera_core::names::file_name(&name) != encoded {
        return None;
    }
    Some(if namespace == "pins" {
        Target::Pin(name)
    } else {
        Target::Lineage(name)
    })
}

/// Render operation-specific guidance without changing the typed error used for classification.
pub fn message(error: &Error, target: Option<&Target>) -> String {
    let recorded_target = ref_target(error);
    let target = target.or(recorded_target.as_ref());
    if let (
        Error::RefMoved {
            expected, found, ..
        },
        Some(Target::Pin(name)),
    ) = (error, target)
    {
        return format!(
            concat!(
                "pin {} moved: expected {}, found {}; no pin was changed — ",
                "reread the pin before setting or removing it again"
            ),
            quote(name),
            expected.as_deref().unwrap_or("no target"),
            found.as_deref().unwrap_or("no target"),
        );
    }
    let mismatch = match error {
        Error::RefTargetUnreadable {
            address, source, ..
        } => Some(format!("target {address} is unreadable: {source}")),
        Error::RefKindMismatch { record, object, .. } => Some(format!(
            "object kind mismatch: record {record:?}, object {object:?}"
        )),
        Error::ObjectKindMismatch {
            expected, object, ..
        } => Some(format!(
            "object kind mismatch: expected {expected:?}, object {object:?}"
        )),
        _ => None,
    };
    if let (Some(detail), Some(target)) = (mismatch, target) {
        let (role, name) = match target {
            Target::Pin(name) => ("pin", name),
            Target::Lineage(name) => ("lineage", name),
            Target::Version(name) => ("version", name),
        };
        return format!("{role} {}: {detail}", quote(name));
    }
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::ObjectKind::{Collection, Product};

    #[test]
    fn stable_error_code_table() {
        let path = || std::path::PathBuf::from("repo/refs/pins/a%3ab");
        let source = || Box::new(Error::Invalid("unreadable".into()));
        macro_rules! cases {
            ($($expected:literal => $error:expr),+ $(,)?) => { $( {
                let error = $error;
                assert_eq!(code(&error), $expected, "{error:?}");
            } )+ };
        }
        cases! {
            3 => Error::BackendNotCompiled("hdf5"),
            4 => Error::RefMoved { lineage: "x".into(), expected: None, found: None },
            5 => Error::LineageKindMismatch { lineage: "x".into(), tip_kind: Product },
            5 => Error::ObjectKindMismatch { path: path(), expected: Collection, object: Product },
            6 => Error::RefKindMismatch { path: path(), record: Collection, object: Product },
            6 => Error::RefTargetUnreadable { path: path(), address: "a".into(),
                source: Box::new(std::io::Error::from(std::io::ErrorKind::NotFound).into()) },
            6 => Error::RefTargetUnreadable { path: path(), address: "a".into(),
                source: Box::new(Error::Integrity { what: "seal", expected: "a".into(), actual: "b".into() }) },
            5 => Error::UnstoredPinTarget { name: "pin".into(), address: "a".into() },
            5 => Error::MissingProductBlock { name: "data".into(), digest: "a".into() },
            5 => Error::MemberKindMismatch { path: "members[0]".into(), expected: Collection, object: Product },
            5 => Error::MissingMember { path: "members[0]".into(), source: source() },
            5 => Error::MissingMember { path: "members[1]".into(),
                source: Box::new(Error::Integrity { what: "seal", expected: "a".into(), actual: "b".into() }) },
            5 => Error::MissingForkToken,
            5 => Error::UnstoredLineageTarget { name: "lineage".into(), address: "a".into() },
            5 => Error::LineageIdMismatch { lineage: "x".into(), expected: "a".into(), found: "b".into() },
            5 => Error::SupersedesMismatch { lineage: "x".into(), expected: "a".into(), found: vec![] },
            5 => Error::InvalidRefName { namespace: "pins".into(), name: "".into(), reason: "empty".into() },
            6 => Error::CorruptRef { path: path(), reason: "torn".into() },
            6 => Error::GcAborted { what: "member".into(), source: source() },
            6 => Error::HistoryAborted { what: "parent".into(), source: source() },
            6 => Error::UnsupportedRepositoryLayout { root: "repo".into(), found: "1".into(), required: "2".into() },
            1 => Error::Invalid("ordinary invalid input".into()),
            1 => Error::AmbiguousObjectKind,
        }
    }

    #[test]
    fn fork_validator_is_classified_without_reclassifying_other_invalid_errors() {
        use tessera_core::collection::CollectionBuilder;
        use tessera_core::provenance::Source;
        for token in [None, Some(""), Some(" \t")] {
            let mut builder = CollectionBuilder::new("fork", "", "2024-01-01T00:00:00Z");
            let mut inputs = std::collections::BTreeMap::new();
            if let Some(token) = token {
                inputs.insert("fork".into(), token.into());
            }
            builder.with_id_inputs(inputs).add_source(
                Source::new("forked_from", "parent")
                    .with_content_hash(tessera_core::hash::digest(b"external predecessor")),
            );
            let error = builder.seal().unwrap_err();
            assert!(matches!(error, Error::MissingForkToken));
            assert_eq!(code(&error), 5);
        }
        assert_eq!(code(&Error::Invalid("other invalid input".into())), 1);
    }

    #[test]
    fn pin_diagnostics_use_the_logical_name_and_pin_guidance() {
        let target = Target::Pin("a:b\n\"c".into());
        let error = Error::RefKindMismatch {
            path: "repo/refs/pins/a%3ab%0a%22c".into(),
            record: Collection,
            object: Product,
        };
        assert_eq!(
            message(&error, Some(&target)),
            "pin \"a:b\\n\\\"c\": object kind mismatch: record Collection, object Product"
        );
        assert_eq!(message(&error, None), message(&error, Some(&target)));
        let error = Error::RefMoved {
            lineage: "a:b\n\"c".into(),
            expected: None,
            found: None,
        };
        let text = message(&error, Some(&target));
        assert!(text.contains("reread the pin"), "{text}");
        assert!(!text.contains("commit"), "{text}");
    }
}
