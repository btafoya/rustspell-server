//! Application configuration loaded from environment variables.

use std::path::PathBuf;

use axum::http::HeaderValue;
use tower_http::cors::AllowOrigin;

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
    /// CORS allow-list.
    pub cors_origins: Vec<HeaderValue>,
}

impl Config {
    /// Build the CORS [`AllowOrigin`] layer from the parsed allow-list.
    pub fn cors_allow_origin(&self) -> AllowOrigin {
        AllowOrigin::list(self.cors_origins.clone())
    }
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

    let cors_origins = parse_cors_origins()?;
    if cors_origins.is_empty() {
        anyhow::bail!("RUSTSPELL_CORS_ORIGINS must contain at least one valid origin");
    }

    let dictionary_dir = std::env::var_os("RUSTSPELL_DICTIONARY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_dictionary_dir);

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
        cors_origins,
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

fn parse_cors_origins() -> anyhow::Result<Vec<HeaderValue>> {
    let raw = std::env::var("RUSTSPELL_CORS_ORIGINS").unwrap_or_default();
    let mut origins = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let value = HeaderValue::from_str(part)
            .map_err(|e| anyhow::anyhow!("invalid CORS origin '{part}': {e}"))?;
        origins.push(value);
    }
    Ok(origins)
}

fn default_dictionary_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "rustspell", "rustspell-server")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("./data"))
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
        std::env::set_var("RUSTSPELL_CORS_ORIGINS", "http://localhost:3000");

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
        assert_eq!(config.cors_origins.len(), 1);
    }

    #[test]
    fn rejects_equal_ports() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_PORT", "3000");
        std::env::set_var("RUSTSPELL_METRICS_PORT", "3000");
        std::env::set_var("RUSTSPELL_CORS_ORIGINS", "http://localhost:3000");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("must be different"));
    }

    #[test]
    fn rejects_missing_cors_origins() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        let err = load().unwrap_err().to_string();
        assert!(err.contains("RUSTSPELL_CORS_ORIGINS"));
    }

    #[test]
    fn parses_multiple_cors_origins() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var(
            "RUSTSPELL_CORS_ORIGINS",
            "http://localhost:3000,https://example.com",
        );

        let config = load().expect("load should succeed");
        assert_eq!(config.cors_origins.len(), 2);
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
            "RUSTSPELL_CORS_ORIGINS",
        ] {
            std::env::remove_var(key);
        }
    }
}
