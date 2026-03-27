//! Deterministic filename generation following the fd5 naming convention.
//!
//! Format: `YYYY-MM-DD_HH-MM-SS_<product>-<id8>.h5`

const SHA256_PREFIX: &str = "sha256:";
const ID_HEX_LENGTH: usize = 8;
const EXTENSION: &str = ".h5";

/// Generate an fd5-compliant filename.
///
/// Format: `YYYY-MM-DD_HH-MM-SS_<product>-<id8>.h5`
///
/// When `timestamp` is `None` the datetime prefix is omitted.
/// The `id_hash` is truncated to the first 8 hex characters; a
/// `sha256:` prefix is stripped automatically if present.
pub fn generate_filename(product: &str, id_hash: &str, timestamp: Option<&str>) -> String {
    let short_id = truncate_id(id_hash);
    let mut parts: Vec<String> = Vec::new();

    if let Some(ts) = timestamp {
        if let Some(formatted) = format_timestamp(ts) {
            parts.push(formatted);
        }
    }

    parts.push(format!("{}-{}", product, short_id));
    format!("{}{}", parts.join("_"), EXTENSION)
}

fn truncate_id(id_hash: &str) -> &str {
    let raw = id_hash.strip_prefix(SHA256_PREFIX).unwrap_or(id_hash);
    &raw[..raw.len().min(ID_HEX_LENGTH)]
}

/// Best-effort reformat of an ISO 8601 timestamp to `YYYY-MM-DD_HH-MM-SS`.
fn format_timestamp(ts: &str) -> Option<String> {
    if ts.is_empty() {
        return None;
    }
    let ts = ts.trim();

    let (date_part, time_part) = if let Some(pos) = ts.find('T') {
        (&ts[..pos], Some(&ts[pos + 1..]))
    } else if let Some(pos) = ts.find(' ') {
        (&ts[..pos], Some(&ts[pos + 1..]))
    } else {
        (ts, None)
    };

    if date_part.len() < 10 {
        return Some(date_part.to_string());
    }

    let date_formatted = &date_part[..10];

    if let Some(time) = time_part {
        // Strip timezone suffix and fractional seconds
        let time_clean = time
            .split(|c: char| c == '+' || c == 'Z')
            .next()
            .unwrap_or(time);
        let time_clean = time_clean
            .split('.')
            .next()
            .unwrap_or(time_clean);
        if time_clean.len() >= 8 {
            let hh = &time_clean[0..2];
            let mm = &time_clean[3..5];
            let ss = &time_clean[6..8];
            Some(format!("{}_{}-{}-{}", date_formatted, hh, mm, ss))
        } else {
            Some(date_formatted.to_string())
        }
    } else {
        Some(date_formatted.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_filename_with_timestamp() {
        let name = generate_filename(
            "imaging/recon",
            "sha256:abcdef0123456789",
            Some("2024-01-15T10:30:00"),
        );
        assert_eq!(name, "2024-01-15_10-30-00_imaging-recon-abcdef01.h5");
    }

    #[test]
    fn test_generate_filename_no_timestamp() {
        let name = generate_filename("test/product", "sha256:deadbeef99", None);
        assert_eq!(name, "test-product-deadbeef.h5");
    }

    #[test]
    fn test_truncate_id_strips_prefix() {
        assert_eq!(truncate_id("sha256:abcdef0123456789"), "abcdef01");
    }

    #[test]
    fn test_truncate_id_no_prefix() {
        assert_eq!(truncate_id("abcdef0123456789"), "abcdef01");
    }
}
