//! Application error type mapped to RFC 7807 `application/problem+json` responses.

use axum::{
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Application-wide error enum.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("validation error")]
    Validation(#[from] validator::ValidationErrors),
    #[error("invalid request body")]
    JsonRejection(#[from] JsonRejection),
    #[error("dictionary download failed: {0}")]
    DictionaryDownload(String),
    #[error("dictionary parse failed: {0}")]
    DictionaryParse(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// RFC 7807 Problem Details object.
#[derive(Debug, Serialize)]
struct ProblemDetails {
    r#type: String,
    title: String,
    status: u16,
    detail: String,
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::Validation(_) | AppError::JsonRejection(_) => StatusCode::BAD_REQUEST,
            AppError::DictionaryDownload(_) | AppError::DictionaryParse(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn problem_type(&self) -> &'static str {
        match self {
            AppError::Validation(_) => "https://rustspell.example/errors/validation-error",
            AppError::JsonRejection(_) => "https://rustspell.example/errors/invalid-json",
            AppError::DictionaryDownload(_) => {
                "https://rustspell.example/errors/dictionary-download-error"
            }
            AppError::DictionaryParse(_) => {
                "https://rustspell.example/errors/dictionary-parse-error"
            }
            AppError::Internal(_) => "https://rustspell.example/errors/internal-error",
        }
    }

    fn title(&self) -> &'static str {
        match self {
            AppError::Validation(_) => "Validation error",
            AppError::JsonRejection(_) => "Invalid JSON",
            AppError::DictionaryDownload(_) => "Dictionary download error",
            AppError::DictionaryParse(_) => "Dictionary parse error",
            AppError::Internal(_) => "Internal server error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ProblemDetails {
            r#type: self.problem_type().to_string(),
            title: self.title().to_string(),
            status: status.as_u16(),
            detail: self.to_string(),
        };

        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            Json(body),
        )
            .into_response()
    }
}

/// Convenience `Result` alias.
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn validation_error_maps_to_bad_request() {
        let err: AppError = validator::ValidationErrors::new().into();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
