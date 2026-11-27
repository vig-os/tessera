//! Shared budget for automatically sized Tessera worker pools.

/// Return the cached thread upper bound, capped at system parallelism.
///
/// Values must contain only ASCII decimal digits and fit in a nonzero `usize`.
/// Invalid values (including non-Unicode values) are logged to stderr and ignored.
/// If the system cannot report its parallelism, use one worker. Set the environment
/// before starting Tessera: the first call caches the budget for the process. A value
/// above system parallelism is clamped with one stderr note, including across repeated calls.
/// This sizes each automatic pool, not the total threads across concurrent pools.
/// Explicit caller-supplied worker counts remain caller-controlled.
pub fn budget() -> usize {
    static BUDGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        let available = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        const INVALID: &str = "invalid TESSERA_MAX_THREADS; ignoring (expected a positive integer)";
        match std::env::var("TESSERA_MAX_THREADS") {
            Ok(value) => match parse_budget(&value) {
                Some(n) => {
                    if n > available {
                        let note = format!(
                            "TESSERA_MAX_THREADS={n} clamped to system parallelism {available}"
                        );
                        eprintln!("{note}"); // guardrails-ok: one startup configuration diagnostic.
                    }
                    return n.min(available);
                }
                None => {
                    eprintln!("{INVALID}: {value:?}"); // guardrails-ok: public configuration diagnostic.
                }
            },
            Err(std::env::VarError::NotUnicode(_)) => {
                eprintln!("{INVALID}: non-Unicode value"); // guardrails-ok: public diagnostic.
            }
            Err(std::env::VarError::NotPresent) => {}
        }
        available
    })
}

fn parse_budget(value: &str) -> Option<usize> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<usize>().ok().filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_budget_and_invalid_value_diagnostic() {
        const CHILD: &str = "TESSERA_CORE_BUDGET_CHILD";
        if let Ok(expected) = std::env::var(CHILD) {
            let n = budget();
            if expected == "fallback" {
                assert!(n > 0);
            } else {
                assert!(n > 0 && n <= expected.parse::<usize>().unwrap());
                assert_eq!(n, budget());
            }
            return;
        }
        for (value, expected) in [
            (Some("2"), "2"),
            (Some("100000"), "100000"),
            (Some("0"), "fallback"),
            (None, "fallback"),
        ] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "threads::tests::environment_budget_and_invalid_value_diagnostic",
                "--nocapture",
            ]);
            command
                .env(CHILD, expected)
                .env_remove("TESSERA_MAX_THREADS");
            if let Some(value) = value {
                command.env("TESSERA_MAX_THREADS", value);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if value == Some("100000") {
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr)
                        .matches("clamped to system parallelism")
                        .count(),
                    1
                );
            }
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).contains("invalid TESSERA_MAX_THREADS"),
                value == Some("0")
            );
        }
    }

    #[test]
    fn accepts_only_positive_representable_decimal_budgets() {
        assert_eq!(parse_budget("2"), Some(2));
        assert_eq!(parse_budget("001"), Some(1));
        assert_eq!(parse_budget(&usize::MAX.to_string()), Some(usize::MAX));
        for invalid in [
            "",
            "0",
            "-1",
            "+2",
            " 2",
            "2 ",
            "1.5",
            "four",
            "999999999999999999999999999999999999999",
        ] {
            assert_eq!(parse_budget(invalid), None, "{invalid:?}");
        }
    }
}
