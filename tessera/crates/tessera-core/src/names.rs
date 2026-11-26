//! Injective encoding for filesystem names shared by collection members and repository refs.

/// Encode UTF-8 bytes as a file-name stem, preserving only `a-z`, `0-9`, `-` and `_`.
/// Every other byte becomes lowercase `%xx`, including `%`, uppercase letters, dots and Unicode.
/// Thus distinct inputs stay distinct on case-insensitive and Unicode-normalizing filesystems.
/// This is the byte-for-byte encoding introduced for lineage refs in #975.
///
/// This pure encoder imposes no length limit and maps empty input to empty output. Callers must
/// validate their own nonempty-name and filesystem component limits including any suffix.
/// Collection content ids (`blake3:` plus 64 hex digits) encode to 73 bytes, leaving room for
/// `.collection.json` and `.collection.seal`. Windows device names are not escaped; Windows is
/// not a supported target (see `dist-workspace.toml`).
pub fn file_name(value: &str) -> String {
    let mut name = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_' {
            name.push(char::from(b));
        } else {
            name.push_str(&format!("%{b:02x}"));
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn encoding_matches_the_original_lineage_mapping() {
        for (input, expected) in [
            ("a:b", "a%3ab"),
            ("a_b", "a_b"),
            ("Ab", "%41b"),
            ("ab", "ab"),
            ("a/b\\c", "a%2fb%5cc"),
            ("%3a", "%253a"),
            ("..", "%2e%2e"),
            ("é", "%c3%a9"),
            ("blake3:0123", "blake3%3a0123"),
            ("az09-_", "az09-_"),
        ] {
            assert_eq!(file_name(input), expected);
        }
        // Exhaust every ASCII byte, including control bytes and all safe bytes.
        for b in 0u8..=127 {
            let encoded = file_name(&char::from(b).to_string());
            let expected = match b {
                b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => char::from(b).to_string(),
                _ => format!("%{b:02x}"),
            };
            assert_eq!(encoded, expected);
        }
    }
}
