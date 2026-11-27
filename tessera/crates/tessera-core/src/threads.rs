//! Shared budget for automatically sized Tessera worker pools.

/// Return the positive `TESSERA_MAX_THREADS` budget, or the system parallelism.
///
/// Values must contain only ASCII decimal digits and fit in a nonzero `usize`.
/// Invalid values (including non-Unicode values) are logged to stderr and ignored.
/// If the system cannot report its parallelism, use one worker. Set the environment
/// before starting Tessera: existing pools retain the budget used at construction.
/// This sizes each automatic pool, not the total threads across concurrent pools.
/// Explicit caller-supplied worker counts remain caller-controlled.
pub fn budget() -> usize {
    const INVALID: &str = "invalid TESSERA_MAX_THREADS; ignoring (expected a positive integer)";
    match std::env::var("TESSERA_MAX_THREADS") {
        Ok(value) => match parse_budget(&value) {
            Some(n) => return n,
            None => {
                eprintln!("{INVALID}: {value:?}"); // guardrails-ok: public configuration diagnostic.
            }
        },
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("{INVALID}: non-Unicode value"); // guardrails-ok: public diagnostic.
        }
        Err(std::env::VarError::NotPresent) => {}
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
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
                assert_eq!(n, expected.parse::<usize>().unwrap());
            }
            return;
        }
        for (value, expected) in [
            (Some("2"), "2"),
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
