//! Application configuration loaded from environment variables.

use std::path::PathBuf;

/// Default public API port.
pub const DEFAULT_PORT: u16 = 3000;
/// Default Prometheus metrics port.
pub const DEFAULT_METRICS_PORT: u16 = 9090;
/// Default log level filter.
pub const DEFAULT_LOG_LEVEL: &str = "info";
/// Default dictionary language.
pub const DEFAULT_LANGUAGE: &str = "en_US";
/// Default dictionary refresh interval in hours.
pub const DEFAULT_REFRESH_INTERVAL_HOURS: u64 = 24;
/// Default base URL for raw `.aff`/`.dic` dictionary files.
pub const DEFAULT_DICTIONARY_URL: &str =
    "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en";
/// Default auth failures allowed per IP per window before a cooldown.
pub const DEFAULT_AUTH_RATE_LIMIT_MAX: u32 = 10;
/// Default sliding window (seconds) for counting auth failures.
pub const DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECONDS: u64 = 60;
/// Default cooldown (seconds) once the failure threshold is exceeded.
pub const DEFAULT_AUTH_RATE_LIMIT_COOLDOWN_SECONDS: u64 = 60;

/// Runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Public API port.
    pub port: u16,
    /// Prometheus metrics port.
    pub metrics_port: u16,
    /// `tracing` env-filter directive.
    pub log_level: String,
    /// Dictionary locale (e.g. `en_US`).
    pub language: String,
    /// Base URL from which to download `{language}.aff` and `{language}.dic`.
    pub dictionary_url: String,
    /// Directory where extracted `.aff`/`.dic` files are cached.
    pub dictionary_dir: PathBuf,
    /// Re-download if local files are older than this many hours.
    pub refresh_interval_hours: u64,
    /// SQLite file for the key/tenant store, used when `db_url` is unset.
    pub db_path: PathBuf,
    /// PostgreSQL connection string. When set, takes precedence over `db_path`.
    pub db_url: Option<String>,
    /// Auth failures allowed per IP per window before a cooldown.
    pub auth_rate_limit_max: u32,
    /// Sliding window (seconds) for counting auth failures.
    pub auth_rate_limit_window_seconds: u64,
    /// Cooldown (seconds) once the failure threshold is exceeded.
    pub auth_rate_limit_cooldown_seconds: u64,
}

/// Load and validate configuration from the environment.
pub fn load() -> anyhow::Result<Config> {
    let port = parse_env_or("RUSTSPELL_PORT", DEFAULT_PORT)?;
    let metrics_port = parse_env_or("RUSTSPELL_METRICS_PORT", DEFAULT_METRICS_PORT)?;

    if port == metrics_port {
        anyhow::bail!(
            "RUSTSPELL_PORT ({port}) and RUSTSPELL_METRICS_PORT ({metrics_port}) must be different"
        );
    }

    let dictionary_dir = std::env::var_os("RUSTSPELL_DICTIONARY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_dictionary_dir);

    let db_path = std::env::var_os("RUSTSPELL_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path);

    let db_url = match std::env::var("RUSTSPELL_DB_URL") {
        Ok(url) if !url.is_empty() => {
            if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                anyhow::bail!("RUSTSPELL_DB_URL must be a postgres:// connection string");
            }
            Some(url)
        }
        _ => None,
    };

    Ok(Config {
        port,
        metrics_port,
        log_level: std::env::var("RUSTSPELL_LOG_LEVEL")
            .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string()),
        language: std::env::var("RUSTSPELL_LANGUAGE")
            .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string()),
        dictionary_url: std::env::var("RUSTSPELL_DICTIONARY_URL")
            .unwrap_or_else(|_| DEFAULT_DICTIONARY_URL.to_string()),
        dictionary_dir,
        refresh_interval_hours: parse_env_or(
            "RUSTSPELL_REFRESH_INTERVAL_HOURS",
            DEFAULT_REFRESH_INTERVAL_HOURS,
        )?,
        db_path,
        db_url,
        auth_rate_limit_max: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_MAX",
            DEFAULT_AUTH_RATE_LIMIT_MAX,
        )?,
        auth_rate_limit_window_seconds: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS",
            DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECONDS,
        )?,
        auth_rate_limit_cooldown_seconds: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS",
            DEFAULT_AUTH_RATE_LIMIT_COOLDOWN_SECONDS,
        )?,
    })
}

fn parse_env_or<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid {name}: {e}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(anyhow::anyhow!("failed to read {name}: {e}")),
    }
}

fn default_dictionary_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "rustspell", "rustspell-server")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn default_db_path() -> PathBuf {
    directories::ProjectDirs::from("com", "rustspell", "rustspell-server")
        .map(|dirs| dirs.data_dir().join("rustspell.db"))
        .unwrap_or_else(|| PathBuf::from("./data/rustspell.db"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    // Environment tests mutate process-global state; serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_are_valid() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();

        let config = load().expect("load should succeed");
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.metrics_port, DEFAULT_METRICS_PORT);
        assert_eq!(config.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.language, DEFAULT_LANGUAGE);
        assert_eq!(
            config.refresh_interval_hours,
            DEFAULT_REFRESH_INTERVAL_HOURS
        );
        assert_eq!(config.dictionary_url, DEFAULT_DICTIONARY_URL);
    }

    #[test]
    fn rejects_equal_ports() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_PORT", "3000");
        std::env::set_var("RUSTSPELL_METRICS_PORT", "3000");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("must be different"));
    }

    #[test]
    fn rejects_non_postgres_db_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_DB_URL", "mysql://localhost/db");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("RUSTSPELL_DB_URL"));
    }

    fn clear_env() {
        for key in [
            "RUSTSPELL_PORT",
            "RUSTSPELL_METRICS_PORT",
            "RUSTSPELL_LOG_LEVEL",
            "RUSTSPELL_LANGUAGE",
            "RUSTSPELL_DICTIONARY_URL",
            "RUSTSPELL_DICTIONARY_DIR",
            "RUSTSPELL_REFRESH_INTERVAL_HOURS",
            "RUSTSPELL_DB_PATH",
            "RUSTSPELL_DB_URL",
        ] {
            std::env::remove_var(key);
        }
    }
}
