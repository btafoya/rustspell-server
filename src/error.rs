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
    #[error("missing or invalid API key")]
    Unauthorized,
    #[error("insufficient permissions for this key")]
    Forbidden,
    #[error("too many failed authentication attempts")]
    RateLimited { retry_after_secs: u64 },
    #[error("resource not found")]
    NotFound,
    #[error("tenant request quota exceeded")]
    QuotaExceeded,
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),
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
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::QuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
            AppError::UnsupportedLanguage(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn problem_type(&self) -> String {
        let slug = match self {
            AppError::Validation(_) => "validation-error",
            AppError::JsonRejection(_) => "invalid-json",
            AppError::DictionaryDownload(_) => "dictionary-download-error",
            AppError::DictionaryParse(_) => "dictionary-parse-error",
            AppError::Internal(_) => "internal-error",
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden => "forbidden",
            AppError::RateLimited { .. } => "rate-limited",
            AppError::NotFound => "not-found",
            AppError::QuotaExceeded => "quota-exceeded",
            AppError::UnsupportedLanguage(_) => "unsupported-language",
        };
        format!("https://github.com/btafoya/rustspell-server/blob/main/docs/errors/{slug}.md")
    }

    fn title(&self) -> &'static str {
        match self {
            AppError::Validation(_) => "Validation error",
            AppError::JsonRejection(_) => "Invalid JSON",
            AppError::DictionaryDownload(_) => "Dictionary download error",
            AppError::DictionaryParse(_) => "Dictionary parse error",
            AppError::Internal(_) => "Internal server error",
            AppError::Unauthorized => "Unauthorized",
            AppError::Forbidden => "Forbidden",
            AppError::RateLimited { .. } => "Too many requests",
            AppError::NotFound => "Not found",
            AppError::QuotaExceeded => "Quota exceeded",
            AppError::UnsupportedLanguage(_) => "Unsupported language",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let retry_after_secs = match &self {
            AppError::RateLimited { retry_after_secs } => Some(*retry_after_secs),
            _ => None,
        };
        let body = ProblemDetails {
            r#type: self.problem_type(),
            title: self.title().to_string(),
            status: status.as_u16(),
            detail: self.to_string(),
        };

        let mut response = (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            Json(body),
        )
            .into_response();

        if let Some(secs) = retry_after_secs {
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }

        response
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

    #[test]
    fn problem_type_uses_documentation_uri() {
        let err: AppError = validator::ValidationErrors::new().into();
        assert!(err
            .problem_type()
            .starts_with("https://github.com/btafoya/rustspell-server/blob/main/docs/errors/"));
    }

    #[test]
    fn quota_exceeded_and_rate_limited_are_distinct_429s() {
        let quota = AppError::QuotaExceeded;
        let rate_limited = AppError::RateLimited {
            retry_after_secs: 30,
        };
        assert_eq!(quota.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(rate_limited.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_ne!(quota.problem_type(), rate_limited.problem_type());
    }
}
