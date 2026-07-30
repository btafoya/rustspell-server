//! Download, cache, and refresh Hunspell `.aff`/`.dic` files.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::Config;

/// Errors from dictionary download/cache operations.
#[derive(Debug, thiserror::Error)]
pub enum DictionaryError {
    #[error("failed to download dictionary from {url}: {source}")]
    Download { url: String, source: reqwest::Error },
    #[error("failed to create cache directory {path}: {source}")]
    CreateDir {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to read cache file {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to write cache file {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manages dictionary cache lifecycle.
pub struct DictionaryManager {
    language: String,
    base_url: String,
    cache_dir: PathBuf,
    refresh_interval: Duration,
}

impl DictionaryManager {
    /// Build a manager from runtime configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            language: config.language.clone(),
            base_url: config.dictionary_url.clone(),
            cache_dir: config.dictionary_dir.join(&config.language),
            refresh_interval: Duration::from_secs(config.refresh_interval_hours * 3600),
        }
    }

    /// Return the paths to the cached `.aff` and `.dic` files, downloading if needed.
    pub async fn ensure_dictionary(&self) -> Result<(PathBuf, PathBuf), DictionaryError> {
        let aff_path = self.cache_dir.join(format!("{}.aff", self.language));
        let dic_path = self.cache_dir.join(format!("{}.dic", self.language));

        if self.is_fresh(&aff_path, &dic_path).await {
            return Ok((aff_path, dic_path));
        }

        tracing::info!(
            "dictionary cache missing or stale; downloading from {}",
            self.base_url
        );

        tokio::fs::create_dir_all(&self.cache_dir)
            .await
            .map_err(|e| DictionaryError::CreateDir {
                path: self.cache_dir.display().to_string(),
                source: e,
            })?;

        let aff_url = format!("{}/{}.aff", self.base_url, self.language);
        let dic_url = format!("{}/{}.dic", self.base_url, self.language);

        let aff_data = self.download(&aff_url).await?;
        let dic_data = self.download(&dic_url).await?;

        self.atomic_write(&aff_path, aff_data).await?;
        self.atomic_write(&dic_path, dic_data).await?;

        Ok((aff_path, dic_path))
    }

    async fn is_fresh(&self, aff_path: &Path, dic_path: &Path) -> bool {
        let both_exist = aff_path.exists() && dic_path.exists();
        if !both_exist {
            return false;
        }

        let now = SystemTime::now();
        let threshold = self.refresh_interval;

        for path in [aff_path, dic_path] {
            match tokio::fs::metadata(path).await {
                Ok(meta) => match meta.modified() {
                    Ok(modified) => {
                        if now.duration_since(modified).unwrap_or_default() >= threshold {
                            return false;
                        }
                    }
                    Err(_) => return false,
                },
                Err(_) => return false,
            }
        }
        true
    }

    /// Retry transient network errors with exponential backoff.
    const RETRY_ATTEMPTS: u32 = 5;
    const RETRY_BASE_MS: u64 = 250;

    async fn download(&self, url: &str) -> Result<Vec<u8>, DictionaryError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("rustspell-server/0.1.0")
            .build()
            .map_err(|e| DictionaryError::Download {
                url: url.to_string(),
                source: e,
            })?;

        let mut last_error = None;
        for attempt in 0..Self::RETRY_ATTEMPTS {
            match Self::download_once(&client, url).await {
                Ok(data) => return Ok(data),
                Err(err) => {
                    if !Self::is_transient_error(&err) || attempt == Self::RETRY_ATTEMPTS - 1 {
                        return Err(DictionaryError::Download {
                            url: url.to_string(),
                            source: err,
                        });
                    }
                    last_error = Some(err);
                    let delay = Duration::from_millis(Self::RETRY_BASE_MS * 2_u64.pow(attempt));
                    tracing::warn!(
                        "dictionary download transient failure (attempt {}), retrying in {:?}: {}",
                        attempt + 1,
                        delay,
                        last_error.as_ref().unwrap()
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Err(DictionaryError::Download {
            url: url.to_string(),
            source: last_error.expect("retry loop must set last_error on transient path"),
        })
    }

    async fn download_once(client: &reqwest::Client, url: &str) -> reqwest::Result<Vec<u8>> {
        let response = client.get(url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    fn is_transient_error(err: &reqwest::Error) -> bool {
        err.is_connect() || err.is_timeout()
    }

    async fn atomic_write(&self, dest: &Path, data: Vec<u8>) -> Result<(), DictionaryError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("rustspell-dict-")
            .tempdir_in(&self.cache_dir)
            .map_err(|e| DictionaryError::Write {
                path: dest.display().to_string(),
                source: e,
            })?;
        let temp_path = temp_dir.path().join("dict.tmp");

        tokio::fs::write(&temp_path, data)
            .await
            .map_err(|e| DictionaryError::Write {
                path: dest.display().to_string(),
                source: e,
            })?;

        tokio::fs::rename(&temp_path, dest)
            .await
            .map_err(|e| DictionaryError::Write {
                path: dest.display().to_string(),
                source: e,
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_config(dict_dir: PathBuf) -> Config {
        Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com/dict".to_string(),
            dictionary_dir: dict_dir,
            refresh_interval_hours: 24,
            cors_origins: vec![],
        }
    }

    #[tokio::test]
    async fn detects_missing_files_as_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let config = fake_config(dir.path().to_path_buf());
        let manager = DictionaryManager::new(&config);

        let aff = dir.path().join("en_US.aff");
        let dic = dir.path().join("en_US.dic");
        assert!(!manager.is_fresh(&aff, &dic).await);
    }

    #[test]
    fn detects_connect_errors_as_transient() {
        let client = reqwest::blocking::Client::new();
        let connect_err = client.get("http://localhost:1").send().unwrap_err();
        assert!(DictionaryManager::is_transient_error(&connect_err));
    }

    #[tokio::test]
    async fn detects_timeout_errors_as_transient() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1))
            .build()
            .unwrap();
        let timeout_err = client
            .get("https://httpbin.org/delay/5")
            .send()
            .await
            .unwrap_err();
        assert!(DictionaryManager::is_transient_error(&timeout_err));
    }

    #[tokio::test]
    async fn status_errors_are_not_transient() {
        // Build a reqwest error that wraps a status code (404) so is_status is true
        // but is_connect/is_timeout are false.
        let status_err = reqwest::get("https://httpbin.org/status/404")
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();
        assert!(status_err.is_status());
        assert!(!DictionaryManager::is_transient_error(&status_err));
    }
}
