//! Thin, thread-safe wrapper around `spellbook::Dictionary` plus a local tokenizer.

use std::path::Path;

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
}
