//! Resource-cap resolution (#368) — the SSoT for turning CLI flags, environment, and an optional
//! config file into a [`WriteConfig`], with a strict **flag > env > conf > default** precedence
//! resolved *per field*. This is the first global tessera config file; it is **additive** — with no
//! flag, no env var, and no config file the result is exactly [`WriteConfig::for_system`], so every
//! existing invocation is unchanged.
//!
//! Precedence, highest first, for each of `workers` and `ram_budget` independently:
//! 1. **flag** — the per-invocation `--workers` / `--ram-budget` (explicit intent wins).
//! 2. **env** — `TESSERA_WORKERS` / `TESSERA_RAM_BUDGET` (the latter takes a human size, e.g. `512MiB`).
//! 3. **conf** — `[resources]` in a config file: repo-local `.tessera/config.toml` first, then the
//!    user file (`$XDG_CONFIG_HOME/tessera/config.toml` or `~/.config/tessera/config.toml`) — the
//!    same repo-then-user layering the trust store uses.
//! 4. **default** — [`WriteConfig::for_system`] (machine-derived `available_parallelism` + 1 GiB).
//!
//! Determinism note: these are *runtime* knobs (thread count, RAM ceiling) — they never change the
//! output bytes (`content_hash` is independent of `workers`/`ram_budget`), so the writer-determinism
//! gate holds regardless of what an operator sets.

use std::path::PathBuf;

use serde::Deserialize;
use tessera_core::{Error, Result};
use tessera_io::{parse_byte_size, WriteConfig};

/// Env var overriding the encode/verify worker count (below an explicit flag, above config/default).
const ENV_WORKERS: &str = "TESSERA_WORKERS";
/// Env var overriding the RAM budget — a human size like `512MiB` (below a flag, above config).
const ENV_RAM_BUDGET: &str = "TESSERA_RAM_BUDGET";

/// The `[resources]` table of a tessera config file. Every field optional — a missing field defers
/// to the next-lower precedence tier.
#[derive(Debug, Default, Deserialize)]
struct ResourcesTable {
    #[serde(default)]
    workers: Option<usize>,
    #[serde(default)]
    ram_budget: Option<String>,
}

/// A tessera config file. Only `[resources]` is read today; unknown tables are ignored so the file
/// can grow other sections without breaking older binaries.
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    resources: ResourcesTable,
}

/// Config file search order (repo before user; first file to set a field wins it): repo-local
/// `.tessera/config.toml`, then `$XDG_CONFIG_HOME/tessera/config.toml` or `~/.config/tessera/config.toml`.
///
/// The repo-local file is resolved **relative to the current directory** — the same cwd-relative
/// convention as the trust store's `.tessera/trust` — so it applies when tessera is run from the repo
/// root; it is not discovered by walking up from a subdirectory.
fn config_files() -> Vec<PathBuf> {
    let mut files = vec![PathBuf::from(".tessera/config.toml")];
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        files.push(base.join("tessera/config.toml"));
    }
    files
}

/// Parse a config file's `[resources]` table. A present-but-malformed file is a hard error — better
/// than silently ignoring a cap the operator deliberately set.
fn parse_resources_toml(text: &str, label: &str) -> Result<ResourcesTable> {
    let cfg: ConfigFile =
        toml::from_str(text).map_err(|e| Error::Invalid(format!("config {label}: {e}")))?;
    Ok(cfg.resources)
}

/// Merge `[resources]` tables in precedence order — the first table to set each field wins it
/// (repo before user). Pure: the file I/O lives in [`config_resources`], so the repo-over-user
/// precedence is unit-testable directly without touching the filesystem.
fn merge_resources(tables: impl IntoIterator<Item = ResourcesTable>) -> ResourcesTable {
    let mut merged = ResourcesTable::default();
    for t in tables {
        if merged.workers.is_none() {
            merged.workers = t.workers;
        }
        if merged.ram_budget.is_none() {
            merged.ram_budget = t.ram_budget;
        }
    }
    merged
}

/// Read the config-file tier across the search path and merge it (repo overrides user). Absent files
/// are skipped; a malformed one errors.
fn config_resources() -> Result<ResourcesTable> {
    let mut tables = Vec::new();
    for path in config_files() {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue, // absent file is fine
        };
        tables.push(parse_resources_toml(&text, &path.display().to_string())?);
    }
    Ok(merge_resources(tables))
}

/// Parse a worker-count string (from a flag, env, or config); a present-but-garbage value — or an
/// explicit `0`, which would silently clamp to 1 — is an error, so a typo'd or nonsensical cap is
/// loud rather than silently ignored. `source` names where the value came from.
fn parse_workers(s: &str, source: &str) -> Result<usize> {
    let n = s
        .trim()
        .parse::<usize>()
        .map_err(|_| Error::Invalid(format!("{source}: expected a positive integer, got {s:?}")))?;
    if n == 0 {
        return Err(Error::Invalid(format!(
            "{source}: worker count must be >= 1, got 0"
        )));
    }
    Ok(n)
}

/// Parse a RAM-budget size string (from a flag, env, or config), labeling the parse error with its
/// `source` — symmetric with [`parse_workers`], so a bad value names where it came from instead of
/// surfacing a bare parser error.
fn parse_ram(s: &str, source: &str) -> Result<u64> {
    parse_byte_size(s).map_err(|e| Error::Invalid(format!("{source}: {e}")))
}

/// The **pure** precedence core: pick each field from the first tier that set it, else the machine
/// default carried by `base`. No process env / cwd access — every input is passed in, so the
/// flag > env > conf > default contract is unit-testable without global state.
fn resolve(
    flag_workers: Option<usize>,
    env_workers: Option<usize>,
    conf_workers: Option<usize>,
    flag_ram: Option<u64>,
    env_ram: Option<u64>,
    conf_ram: Option<u64>,
    base: WriteConfig,
) -> WriteConfig {
    let workers = flag_workers
        .or(env_workers)
        .or(conf_workers)
        .unwrap_or_else(|| base.worker_count());
    let ram = flag_ram
        .or(env_ram)
        .or(conf_ram)
        .unwrap_or_else(|| base.ram_budget_bytes());
    base.workers(workers).ram_budget(ram)
}

/// Resolve the [`WriteConfig`] with strict per-field **flag > env > conf > default** precedence. With
/// no flag, no env, and no config file this returns exactly [`WriteConfig::for_system`].
pub fn resolve_write_config(
    flag_workers: Option<usize>,
    flag_ram: Option<&str>,
) -> Result<WriteConfig> {
    let conf = config_resources()?;

    let env_workers = match std::env::var(ENV_WORKERS) {
        Ok(s) => Some(parse_workers(&s, ENV_WORKERS)?),
        Err(_) => None,
    };

    let flag_ram = match flag_ram {
        Some(s) => Some(parse_ram(s, "--ram-budget")?),
        None => None,
    };
    let env_ram = match std::env::var(ENV_RAM_BUDGET) {
        Ok(s) => Some(parse_ram(&s, ENV_RAM_BUDGET)?),
        Err(_) => None,
    };
    let conf_ram = match &conf.ram_budget {
        Some(s) => Some(parse_ram(s, "config [resources].ram_budget")?),
        None => None,
    };

    Ok(resolve(
        flag_workers,
        env_workers,
        conf.workers,
        flag_ram,
        env_ram,
        conf_ram,
        WriteConfig::for_system(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_flag_over_env_over_conf_over_default_per_field() {
        // A base whose defaults (7 workers / 700 bytes) are distinct from every tier value, so the
        // "fell through to default" case is unambiguous.
        let base = WriteConfig::for_system().workers(7).ram_budget(700);

        // flag wins over everything.
        let c = resolve(
            Some(1),
            Some(2),
            Some(3),
            Some(10),
            Some(20),
            Some(30),
            base,
        );
        assert_eq!(c.worker_count(), 1);
        assert_eq!(c.ram_budget_bytes(), 10);

        // env wins when no flag.
        let c = resolve(None, Some(2), Some(3), None, Some(20), Some(30), base);
        assert_eq!(c.worker_count(), 2);
        assert_eq!(c.ram_budget_bytes(), 20);

        // conf wins when no flag/env.
        let c = resolve(None, None, Some(3), None, None, Some(30), base);
        assert_eq!(c.worker_count(), 3);
        assert_eq!(c.ram_budget_bytes(), 30);

        // default (base) when nothing is set.
        let c = resolve(None, None, None, None, None, None, base);
        assert_eq!(c.worker_count(), 7);
        assert_eq!(c.ram_budget_bytes(), 700);

        // the two fields resolve INDEPENDENTLY — workers from env, ram from conf.
        let c = resolve(None, Some(2), Some(3), None, None, Some(30), base);
        assert_eq!(c.worker_count(), 2);
        assert_eq!(c.ram_budget_bytes(), 30);
    }

    #[test]
    fn config_toml_reads_the_resources_table() {
        let t = parse_resources_toml(
            "[resources]\nworkers = 4\nram_budget = \"512MiB\"\n",
            "test",
        )
        .unwrap();
        assert_eq!(t.workers, Some(4));
        assert_eq!(t.ram_budget.as_deref(), Some("512MiB"));

        // an empty / section-less file is fine (all None → defers to lower tiers).
        let empty = parse_resources_toml("", "test").unwrap();
        assert_eq!(empty.workers, None);
        assert_eq!(empty.ram_budget, None);

        // a malformed file is a hard error, not a silent default.
        assert!(parse_resources_toml("[resources]\nworkers = \"not a number\"\n", "test").is_err());
    }

    #[test]
    fn worker_string_parse_rejects_garbage_zero_and_empty() {
        assert_eq!(parse_workers("8", "env").unwrap(), 8);
        assert_eq!(parse_workers("  3 ", "env").unwrap(), 3);
        assert!(parse_workers("lots", "env").is_err());
        // an explicit 0 (which WriteConfig would silently clamp to 1) is rejected loudly.
        assert!(parse_workers("0", "env").is_err());
        // empty / whitespace-only is a garbage cap, not "unset".
        assert!(parse_workers("", "env").is_err());
        assert!(parse_workers("   ", "env").is_err());
    }

    #[test]
    fn config_merge_is_repo_over_user_per_field() {
        // repo sets only workers; user sets both. Repo wins workers; user fills the ram the repo
        // left unset — the repo-over-user, first-to-set-wins, per-field contract.
        let repo = ResourcesTable {
            workers: Some(2),
            ram_budget: None,
        };
        let user = ResourcesTable {
            workers: Some(9),
            ram_budget: Some("1GiB".into()),
        };
        let m = merge_resources([repo, user]);
        assert_eq!(m.workers, Some(2), "repo workers win over user");
        assert_eq!(
            m.ram_budget.as_deref(),
            Some("1GiB"),
            "user ram fills where repo left it unset"
        );
    }

    #[test]
    fn ram_parse_error_names_its_source() {
        let e = parse_ram("not-a-size", ENV_RAM_BUDGET).unwrap_err();
        assert!(
            format!("{e}").contains(ENV_RAM_BUDGET),
            "the error must name where the bad value came from, got: {e}"
        );
        assert_eq!(parse_ram("512MiB", "flag").unwrap(), 512 * 1024 * 1024);
    }
}
