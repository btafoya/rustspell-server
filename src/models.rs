//! Serde request/response models and validation constraints.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

/// Request body for spell-check endpoints.
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "has_input_validation"))]
pub struct SpellCheckRequest {
    /// Free-form text to tokenize and check.
    #[validate(length(min = 0, max = 10_000))]
    pub text: Option<String>,

    /// Explicit list of words to check.
    #[validate(length(min = 0, max = 1_000))]
    pub words: Option<Vec<String>>,
}

fn has_input_validation(req: &SpellCheckRequest) -> Result<(), ValidationError> {
    if req.text.is_none() && req.words.is_none() {
        let mut err = ValidationError::new("missing_input");
        err.message = Some(std::borrow::Cow::Borrowed(
            "Either 'text' or 'words' must be provided",
        ));
        return Err(err);
    }
    Ok(())
}

/// Per-token result returned by `POST /spellcheck`.
#[derive(Debug, Serialize)]
pub struct TokenResult {
    pub token: String,
    pub valid: bool,
    pub suggestions: Vec<String>,
}

/// Response for `POST /spellcheck`.
#[derive(Debug, Serialize)]
pub struct SpellCheckResponse {
    pub results: Vec<TokenResult>,
}

/// Misspelled token with its positions in the combined input.
#[derive(Debug, Serialize)]
pub struct PositionResult {
    pub token: String,
    pub positions: Vec<usize>,
    pub suggestions: Vec<String>,
}

/// Response for `POST /spellcheck/positions`.
#[derive(Debug, Serialize)]
pub struct PositionsResponse {
    pub results: Vec<PositionResult>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    #[test]
    fn request_with_text_is_valid() {
        let req = SpellCheckRequest {
            text: Some("hello world".to_string()),
            words: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn request_with_both_fields_is_valid() {
        let req = SpellCheckRequest {
            text: Some("hello".to_string()),
            words: Some(vec!["world".to_string()]),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn request_without_input_fails_validation() {
        let req = SpellCheckRequest {
            text: None,
            words: None,
        };
        let err = req.validate().unwrap_err();
        assert!(err.to_string().contains("Either 'text' or 'words'"));
    }
}
