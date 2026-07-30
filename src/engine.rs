//! Thin, thread-safe wrapper around `spellbook::Dictionary` plus a local tokenizer.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use regex::Regex;

/// Errors that can occur when loading or using the engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to read dictionary file: {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to parse dictionary")]
    Parse,
    #[error("dictionary unavailable: {0}")]
    Dictionary(#[from] crate::dictionary::DictionaryError),
}

/// A token extracted from input text, with its original form and char position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The token after stripping surrounding punctuation.
    pub token: String,
    /// Byte index into the original input where the token starts.
    pub start_byte: usize,
    /// Byte index into the original input where the token ends (exclusive).
    pub end_byte: usize,
    /// Character index into the original input where the token starts.
    pub start_char: usize,
}

/// Thread-safe spell-check engine.
pub struct Engine {
    dict: spellbook::Dictionary,
    token_re: Regex,
}

impl Engine {
    /// Create an engine from raw `.aff` and `.dic` contents.
    pub fn new(aff: &str, dic: &str) -> Result<Self, EngineError> {
        let dict = spellbook::Dictionary::new(aff, dic).map_err(|_| EngineError::Parse)?;
        let token_re = token_regex();
        Ok(Self { dict, token_re })
    }

    /// Load an engine from `.aff` and `.dic` file paths.
    pub fn load_from_paths(aff_path: &Path, dic_path: &Path) -> Result<Self, EngineError> {
        let aff = std::fs::read_to_string(aff_path).map_err(|e| EngineError::Read {
            path: aff_path.display().to_string(),
            source: e,
        })?;
        let dic = std::fs::read_to_string(dic_path).map_err(|e| EngineError::Read {
            path: dic_path.display().to_string(),
            source: e,
        })?;
        Self::new(&aff, &dic)
    }

    /// Check whether a single word is valid.
    pub fn check(&self, word: &str) -> bool {
        self.dict.check(word)
    }

    /// Generate spelling suggestions for a word.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.dict.suggest(word, &mut out);
        out
    }

    /// Tokenize a text string into words, stripping surrounding punctuation.
    ///
    /// Splits on Unicode whitespace and preserves the char position of each
    /// occurrence in the original input.
    pub fn tokenize(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        for mat in self.token_re.find_iter(text) {
            let raw = mat.as_str();
            let stripped = strip_surrounding_punctuation(raw);
            if !stripped.is_empty() {
                tokens.push(Token {
                    token: stripped.to_string(),
                    start_byte: mat.start(),
                    end_byte: mat.end(),
                    start_char: text[..mat.start()].chars().count(),
                });
            }
        }
        tokens
    }
}

/// Cache of loaded [`Engine`]s keyed by language, backed by [`DictionaryManager`]
/// for on-demand download/load of languages beyond the server's startup
/// default. See `DESIGN.md` §25.
pub struct EngineRegistry {
    default_language: String,
    dictionary_manager: crate::dictionary::DictionaryManager,
    engines: RwLock<HashMap<String, Arc<Engine>>>,
    /// Serializes the cache-miss (download+load) path only — hot-path cache
    /// hits above never touch this. A `tokio::sync::Mutex` rather than
    /// `std::sync::Mutex` because it's held across the download `.await`;
    /// holding a `std` lock across an await point would block the async
    /// executor, a real bug, not just a style nit.
    ///
    /// One global lock rather than per-language: simpler, and the only cost
    /// is that two *different* never-before-seen languages requested at the
    /// same instant load one after another instead of in parallel — a rare,
    /// one-time-per-language cold-path cost, not worth a per-language lock
    /// registry (with its own cleanup/growth concerns) to avoid.
    load_lock: tokio::sync::Mutex<()>,
}

impl EngineRegistry {
    pub fn new(
        default_language: String,
        engine: Engine,
        dictionary_manager: crate::dictionary::DictionaryManager,
    ) -> Self {
        let mut engines = HashMap::new();
        engines.insert(default_language.clone(), Arc::new(engine));
        Self {
            default_language,
            dictionary_manager,
            engines: RwLock::new(engines),
            load_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The engine for the server's configured default language. Always
    /// present — it's loaded eagerly at startup (fail-fast, unchanged from
    /// before `EngineRegistry` existed).
    pub fn default_engine(&self) -> Arc<Engine> {
        self.engines
            .read()
            .unwrap()
            .get(&self.default_language)
            .cloned()
            .expect("default language engine is always preloaded")
    }

    /// Cache hit: `O(1)`, no I/O, no lock contention with in-flight loads of
    /// other languages. Cache miss: downloads + parses under `load_lock`,
    /// then caches. Two concurrent misses for the *same* language: the
    /// second blocks on `load_lock` until the first finishes, then its
    /// post-lock re-check finds the entry already cached and returns it
    /// without downloading a second time.
    pub async fn get_or_load(&self, language: &str) -> Result<Arc<Engine>, EngineError> {
        if let Some(engine) = self.engines.read().unwrap().get(language) {
            return Ok(engine.clone());
        }

        let _guard = self.load_lock.lock().await;

        // Re-check: another task may have finished loading this language
        // while we were waiting for the lock.
        if let Some(engine) = self.engines.read().unwrap().get(language) {
            return Ok(engine.clone());
        }

        let (aff_path, dic_path) = self.dictionary_manager.ensure_dictionary(language).await?;
        let engine = Arc::new(Engine::load_from_paths(&aff_path, &dic_path)?);
        self.engines
            .write()
            .unwrap()
            .insert(language.to_string(), engine.clone());
        Ok(engine)
    }
}

fn token_regex() -> Regex {
    // Matches maximal runs of non-whitespace characters.
    Regex::new(r"\S+").expect("static regex is valid")
}

fn strip_surrounding_punctuation(s: &str) -> &str {
    let punctuation: &[char] = &[
        '.', ',', ';', ':', '!', '?', '"', '\'', '(', ')', '[', ']', '{', '}',
    ];
    let start = s
        .char_indices()
        .find(|(_, c)| !punctuation.contains(c))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let end = s[start..]
        .char_indices()
        .rev()
        .find(|(_, c)| !punctuation.contains(c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start);
    &s[start..start + end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_engine() -> Engine {
        // Minimal valid dictionary: every lowercase ASCII word is accepted.
        let aff = r"SET UTF-8
TRY abc
";
        let dic = r"2
hello
world
";
        Engine::new(aff, dic).expect("fixture engine should parse")
    }

    #[test]
    fn check_known_words() {
        let engine = fixture_engine();
        assert!(engine.check("hello"));
        assert!(engine.check("world"));
        assert!(!engine.check("wrld"));
    }

    #[test]
    fn tokenize_strips_punctuation_and_preserves_positions() {
        let engine = fixture_engine();
        let tokens = engine.tokenize("Hello, world! (test)");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].token, "Hello");
        assert_eq!(tokens[0].start_char, 0);
        assert_eq!(tokens[1].token, "world");
        assert_eq!(tokens[1].start_char, 7);
        assert_eq!(tokens[2].token, "test");
        assert_eq!(tokens[2].start_char, 14);
    }

    #[test]
    fn tokenize_handles_unicode() {
        let engine = fixture_engine();
        let tokens = engine.tokenize("café, (rien)");
        assert_eq!(tokens[0].token, "café");
        assert_eq!(tokens[0].start_char, 0);
        assert_eq!(tokens[1].token, "rien");
        assert_eq!(tokens[1].start_char, 6); // raw token '(' position preserved
    }

    #[test]
    fn suggest_returns_results() {
        let engine = fixture_engine();
        let suggestions = engine.suggest("wrld");
        assert!(suggestions.contains(&"world".to_string()));
    }

    fn test_config(dictionary_dir: std::path::PathBuf) -> crate::config::Config {
        crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            // Unreachable-for-dictionaries but fast-failing host: any
            // download attempt 404s immediately (not a transient error, so
            // no retry backoff), keeping "unloadable language" tests quick.
            dictionary_url: "https://example.com/no-such-dictionaries".to_string(),
            dictionary_dir,
            refresh_interval_hours: 24,
            db_path: std::path::PathBuf::from("/tmp/rustspell-engine-test.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        }
    }

    /// Writes a fixture `.aff`/`.dic` pair at the cache path
    /// `DictionaryManager::ensure_dictionary` expects for `language`, so
    /// loading it never touches the network.
    fn write_cached_fixture(dictionary_dir: &std::path::Path, language: &str) {
        let lang_dir = dictionary_dir.join(language);
        std::fs::create_dir_all(&lang_dir).unwrap();
        std::fs::write(
            lang_dir.join(format!("{language}.aff")),
            "SET UTF-8\nTRY abc\n",
        )
        .unwrap();
        std::fs::write(lang_dir.join(format!("{language}.dic")), "1\nbonjour\n").unwrap();
    }

    #[tokio::test]
    async fn get_or_load_returns_default_engine_for_default_language() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        let dictionary_manager = crate::dictionary::DictionaryManager::new(&config);
        let registry = EngineRegistry::new(
            config.language.clone(),
            fixture_engine(),
            dictionary_manager,
        );

        let via_default = registry.default_engine();
        let via_get_or_load = registry.get_or_load(&config.language).await.unwrap();
        assert!(Arc::ptr_eq(&via_default, &via_get_or_load));
    }

    #[tokio::test]
    async fn get_or_load_serves_a_cached_non_default_language_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        write_cached_fixture(dir.path(), "fr_FR");
        let dictionary_manager = crate::dictionary::DictionaryManager::new(&config);
        let registry = EngineRegistry::new(
            config.language.clone(),
            fixture_engine(),
            dictionary_manager,
        );

        let engine = registry.get_or_load("fr_FR").await.unwrap();
        assert!(engine.check("bonjour"));
        assert!(!engine.check("hello")); // fr_FR fixture only knows "bonjour"
    }

    #[tokio::test]
    async fn get_or_load_fails_for_unloadable_language() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        let dictionary_manager = crate::dictionary::DictionaryManager::new(&config);
        let registry = EngineRegistry::new(
            config.language.clone(),
            fixture_engine(),
            dictionary_manager,
        );

        let result = registry.get_or_load("xx_NOPE").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_or_load_concurrent_calls_for_new_language_share_one_engine() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        write_cached_fixture(dir.path(), "de_DE");
        let dictionary_manager = crate::dictionary::DictionaryManager::new(&config);
        let registry = Arc::new(EngineRegistry::new(
            config.language.clone(),
            fixture_engine(),
            dictionary_manager,
        ));

        let r1 = registry.clone();
        let r2 = registry.clone();
        let (e1, e2) = tokio::join!(
            tokio::spawn(async move { r1.get_or_load("de_DE").await }),
            tokio::spawn(async move { r2.get_or_load("de_DE").await }),
        );
        let e1 = e1.unwrap().unwrap();
        let e2 = e2.unwrap().unwrap();
        assert!(
            Arc::ptr_eq(&e1, &e2),
            "concurrent loads of the same new language must converge to one cached Engine"
        );
    }
}
