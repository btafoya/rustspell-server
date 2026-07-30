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

    async fn download(&self, url: &str) -> Result<Vec<u8>, DictionaryError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("rustspell-server/0.1.0")
            .build()
            .map_err(|e| DictionaryError::Download {
                url: url.to_string(),
                source: e,
            })?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| DictionaryError::Download {
                url: url.to_string(),
                source: e,
            })?;

        let response = response
            .error_for_status()
            .map_err(|e| DictionaryError::Download {
                url: url.to_string(),
                source: e,
            })?;

        let bytes = response
            .bytes()
            .await
            .map_err(|e| DictionaryError::Download {
                url: url.to_string(),
                source: e,
            })?;

        Ok(bytes.to_vec())
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
}
