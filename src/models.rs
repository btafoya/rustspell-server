//! Serde request/response models and validation constraints.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::store::{DictionaryInfo, KeyRecord, OriginInfo, Role, TenantInfo};

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

    /// Overrides the calling tenant's default language for this request.
    /// Omit to use the tenant's configured language.
    pub language: Option<String>,
}

fn has_input_validation(req: &SpellCheckRequest) -> Result<(), ValidationError> {
    if req.text.is_none() && req.words.is_none() {
        let mut err = ValidationError::new("missing_input");
        err.message = Some(std::borrow::Cow::Borrowed(
            "Either 'text' or 'words' must be provided",
        ));
        return Err(err);
    }
    if let Some(language) = &req.language {
        if !is_valid_language_code(language) {
            let mut err = ValidationError::new("invalid_language");
            err.message = Some(std::borrow::Cow::Borrowed(
                "language must be 1-20 characters of letters, digits, underscore, or hyphen",
            ));
            return Err(err);
        }
    }
    Ok(())
}

/// Conservative charset, not full BCP-47 — this exists primarily to stop a
/// `language` value from being usable for path traversal or URL injection
/// when it's later interpolated into a cache directory path and a
/// dictionary-download URL (`EngineRegistry::get_or_load`).
fn is_valid_language_code(language: &str) -> bool {
    !language.is_empty()
        && language.len() <= 20
        && language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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

/// Request body for `POST /api-keys`.
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_create_api_key_request"))]
pub struct CreateApiKeyRequest {
    #[validate(length(min = 1, max = 100))]
    pub label: String,
    pub role: Role,
    /// Unix-epoch seconds; must be in the future if present.
    pub expires_at: Option<u64>,
}

fn validate_create_api_key_request(req: &CreateApiKeyRequest) -> Result<(), ValidationError> {
    if req.role == Role::Platform {
        let mut err = ValidationError::new("invalid_role");
        err.message = Some(std::borrow::Cow::Borrowed(
            "role must be 'admin' or 'standard'",
        ));
        return Err(err);
    }
    Ok(())
}

/// Public metadata for one API key. Never includes the raw or hashed value.
#[derive(Debug, Serialize)]
pub struct ApiKeyMetadata {
    pub id: String,
    pub label: String,
    pub role: Role,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
}

impl From<&KeyRecord> for ApiKeyMetadata {
    fn from(record: &KeyRecord) -> Self {
        Self {
            id: record.id.clone(),
            label: record.label.clone(),
            role: record.role,
            created_at: record.created_at,
            expires_at: record.expires_at,
            last_used_at: record.last_used_at,
            revoked_at: record.revoked_at,
        }
    }
}

/// Response for `POST /api-keys` and `POST /api-keys/{id}/rotate`. The raw
/// key is returned exactly once, here — it is never persisted or shown again.
#[derive(Debug, Serialize)]
pub struct CreatedApiKeyResponse {
    #[serde(flatten)]
    pub metadata: ApiKeyMetadata,
    pub key: String,
}

/// Response for `GET /api-keys`.
#[derive(Debug, Serialize)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyMetadata>,
}

/// Request body for `POST /tenants` (platform key only).
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_create_tenant_request"))]
pub struct CreateTenantRequest {
    #[validate(length(min = 1, max = 200))]
    pub name: String,
    pub language: Option<String>,
    pub quota_limit: Option<u64>,
    pub period_start: Option<u64>,
    pub period_end: Option<u64>,
}

fn validate_create_tenant_request(req: &CreateTenantRequest) -> Result<(), ValidationError> {
    if let Some(language) = &req.language {
        if !is_valid_language_code(language) {
            let mut err = ValidationError::new("invalid_language");
            err.message = Some(std::borrow::Cow::Borrowed(
                "language must be 1-20 characters of letters, digits, underscore, or hyphen",
            ));
            return Err(err);
        }
    }
    Ok(())
}

/// Request body for `PATCH /tenants/{id}` (platform key only). `period_start`
/// and `period_end` are tri-state via `Option<Option<u64>>`: an absent field
/// leaves the value unchanged, `null` clears it, a number sets it.
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_update_tenant_request"))]
pub struct UpdateTenantRequest {
    #[validate(length(min = 1, max = 200))]
    pub name: Option<String>,
    pub language: Option<String>,
    pub quota_limit: Option<u64>,
    /// Resets usage, e.g. to `0` on a billing period rollover (F46). Not
    /// tri-state like the period fields — there's no meaningful "clear".
    pub request_count: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_present_as_some")]
    pub period_start: Option<Option<u64>>,
    #[serde(default, deserialize_with = "deserialize_present_as_some")]
    pub period_end: Option<Option<u64>>,
}

fn validate_update_tenant_request(req: &UpdateTenantRequest) -> Result<(), ValidationError> {
    if let Some(language) = &req.language {
        if !is_valid_language_code(language) {
            let mut err = ValidationError::new("invalid_language");
            err.message = Some(std::borrow::Cow::Borrowed(
                "language must be 1-20 characters of letters, digits, underscore, or hyphen",
            ));
            return Err(err);
        }
    }
    Ok(())
}

/// Plain `Option<Option<T>>` collapses "field present but null" and "field
/// absent" to the same `None` — serde's `Option<T>` deserializer treats any
/// `null` it sees as `None`, one level up from where we need it. This makes
/// a *present* field (json `null` or a value) go through the inner
/// `Option<T>`'s own null-handling instead, wrapped in an outer `Some`;
/// `#[serde(default)]` handles the absent case by never calling this at all.
fn deserialize_present_as_some<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// Public metadata for one tenant.
#[derive(Debug, Serialize)]
pub struct TenantMetadata {
    pub id: String,
    pub name: String,
    pub language: String,
    pub quota_limit: u64,
    pub request_count: u64,
    pub period_start: Option<u64>,
    pub period_end: Option<u64>,
    pub suspended_at: Option<u64>,
    pub created_at: u64,
}

impl From<&TenantInfo> for TenantMetadata {
    fn from(t: &TenantInfo) -> Self {
        Self {
            id: t.id.clone(),
            name: t.name.clone(),
            language: t.language.clone(),
            quota_limit: t.quota_limit,
            request_count: t.request_count,
            period_start: t.period_start,
            period_end: t.period_end,
            suspended_at: t.suspended_at,
            created_at: t.created_at,
        }
    }
}

/// Response for `POST /tenants`: the tenant plus its first admin key,
/// returned exactly once (mirrors [`CreatedApiKeyResponse`]).
#[derive(Debug, Serialize)]
pub struct CreatedTenant {
    #[serde(flatten)]
    pub tenant: TenantMetadata,
    pub admin_key: CreatedApiKeyResponse,
}

/// Response for `GET /tenants`.
#[derive(Debug, Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantMetadata>,
}

/// Request body for `POST /tenant/origins` (admin key only).
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_register_origin_request"))]
pub struct RegisterOriginRequest {
    pub origin: String,
}

fn validate_register_origin_request(req: &RegisterOriginRequest) -> Result<(), ValidationError> {
    let valid = req.origin.parse::<axum::http::Uri>().is_ok_and(|uri| {
        matches!(uri.scheme_str(), Some("http") | Some("https"))
            && uri.authority().is_some()
            && matches!(uri.path(), "" | "/")
    });
    if !valid {
        let mut err = ValidationError::new("invalid_origin");
        err.message = Some(std::borrow::Cow::Borrowed(
            "origin must be a valid http(s) origin with no path, e.g. https://app.example.com",
        ));
        return Err(err);
    }
    Ok(())
}

/// Public metadata for one registered origin.
#[derive(Debug, Serialize)]
pub struct OriginMetadata {
    pub id: String,
    pub origin: String,
    pub created_at: u64,
}

impl From<&OriginInfo> for OriginMetadata {
    fn from(o: &OriginInfo) -> Self {
        Self {
            id: o.id.clone(),
            origin: o.origin.clone(),
            created_at: o.created_at,
        }
    }
}

/// Response for `GET /tenant/origins`.
#[derive(Debug, Serialize)]
pub struct OriginListResponse {
    pub origins: Vec<OriginMetadata>,
}

/// Request body for `POST /dictionaries` (platform key + admin CIDR only).
#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_add_dictionary_request"))]
pub struct AddDictionaryRequest {
    pub code: String,
    pub source_url: String,
}

fn validate_add_dictionary_request(req: &AddDictionaryRequest) -> Result<(), ValidationError> {
    if !is_valid_language_code(&req.code) {
        let mut err = ValidationError::new("invalid_code");
        err.message = Some(std::borrow::Cow::Borrowed(
            "code must be 1-20 characters of letters, digits, underscore, or hyphen",
        ));
        return Err(err);
    }
    let valid_url = req.source_url.parse::<axum::http::Uri>().is_ok_and(|uri| {
        matches!(uri.scheme_str(), Some("http") | Some("https")) && uri.authority().is_some()
    });
    if !valid_url {
        let mut err = ValidationError::new("invalid_source_url");
        err.message = Some(std::borrow::Cow::Borrowed(
            "source_url must be a valid http(s) URL",
        ));
        return Err(err);
    }
    Ok(())
}

/// Public metadata for one registered dictionary.
#[derive(Debug, Serialize)]
pub struct DictionaryMetadata {
    pub code: String,
    pub source_url: String,
    pub created_at: u64,
}

impl From<&DictionaryInfo> for DictionaryMetadata {
    fn from(info: &DictionaryInfo) -> Self {
        Self {
            code: info.code.clone(),
            source_url: info.source_url.clone(),
            created_at: info.created_at,
        }
    }
}

/// One available language as seen by `GET /languages`.
#[derive(Debug, Serialize)]
pub struct LanguageInfo {
    pub code: String,
    /// Whether the files are already present in the local cache.
    pub cached: bool,
    /// Whether the locale is registered in the dictionary store.
    pub registered: bool,
}

/// Response for `GET /languages`.
#[derive(Debug, Serialize)]
pub struct LanguageListResponse {
    pub languages: Vec<LanguageInfo>,
}

/// Optional window shared by every `/usage/*` endpoint. Supplying exactly one
/// of the two is rejected (F58); omitting both selects the scope-dependent
/// default window (F59).
#[derive(Debug, Default, Deserialize)]
pub struct UsageQuery {
    pub start: Option<String>,
    pub end: Option<String>,
}

/// One day in `GET /usage/daily`. Always dated — F58's carve-out.
#[derive(Debug, Serialize)]
pub struct DailyUsageRow {
    pub date: String,
    pub requests: u64,
    pub average_latency_ms: u64,
    pub errors: u64,
}

#[derive(Debug, Serialize)]
pub struct DailyUsageResponse {
    pub daily_usage: Vec<DailyUsageRow>,
}

/// One percentile in `GET /usage/latency`. `date` is present only when the
/// caller supplied an explicit window.
#[derive(Debug, Serialize)]
pub struct LatencyTrend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub percentile: String,
    pub value_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct LatencyResponse {
    pub latency_trends: Vec<LatencyTrend>,
}

/// One error group in `GET /usage/errors`, carrying both dimensions (F56).
/// `error_code` is the `AppError` slug, identical to the tail of the RFC 7807
/// `type` URI, so a dashboard can link straight to the docs page.
#[derive(Debug, Serialize)]
pub struct ErrorTrend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub status: u16,
    pub error_code: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct ErrorsResponse {
    pub error_trends: Vec<ErrorTrend>,
}

/// One language group in `GET /usage/languages`.
#[derive(Debug, Serialize)]
pub struct LanguageDistribution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub language: String,
    pub count: u64,
    pub percentage: f64,
}

#[derive(Debug, Serialize)]
pub struct LanguagesResponse {
    pub language_distribution: Vec<LanguageDistribution>,
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
            language: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn request_with_both_fields_is_valid() {
        let req = SpellCheckRequest {
            text: Some("hello".to_string()),
            words: Some(vec!["world".to_string()]),
            language: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn request_without_input_fails_validation() {
        let req = SpellCheckRequest {
            text: None,
            words: None,
            language: None,
        };
        let err = req.validate().unwrap_err();
        assert!(err.to_string().contains("Either 'text' or 'words'"));
    }

    #[test]
    fn create_api_key_request_rejects_platform_role() {
        let req = CreateApiKeyRequest {
            label: "test".to_string(),
            role: Role::Platform,
            expires_at: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn create_api_key_request_accepts_admin_and_standard() {
        for role in [Role::Admin, Role::Standard] {
            let req = CreateApiKeyRequest {
                label: "test".to_string(),
                role,
                expires_at: None,
            };
            assert!(req.validate().is_ok());
        }
    }

    #[test]
    fn register_origin_request_accepts_bare_https_origin() {
        let req = RegisterOriginRequest {
            origin: "https://app.example.com".to_string(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn register_origin_request_rejects_path() {
        let req = RegisterOriginRequest {
            origin: "https://app.example.com/callback".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn register_origin_request_rejects_non_http_scheme() {
        let req = RegisterOriginRequest {
            origin: "ftp://app.example.com".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn update_tenant_request_period_fields_are_tri_state() {
        let absent: UpdateTenantRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.period_start, None);

        let explicit_null: UpdateTenantRequest =
            serde_json::from_str(r#"{"period_start": null}"#).unwrap();
        assert_eq!(explicit_null.period_start, Some(None));

        let set: UpdateTenantRequest = serde_json::from_str(r#"{"period_start": 123}"#).unwrap();
        assert_eq!(set.period_start, Some(Some(123)));
    }

    #[test]
    fn create_tenant_request_rejects_malformed_language() {
        let req = CreateTenantRequest {
            name: "Acme".to_string(),
            language: Some("../../etc/passwd".to_string()),
            quota_limit: None,
            period_start: None,
            period_end: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn update_tenant_request_rejects_malformed_language() {
        let req = UpdateTenantRequest {
            name: None,
            language: Some("not valid!".to_string()),
            quota_limit: None,
            request_count: None,
            period_start: None,
            period_end: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn add_dictionary_request_accepts_valid_code_and_url() {
        let req = AddDictionaryRequest {
            code: "fr_FR".to_string(),
            source_url: "https://example.com/dict".to_string(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn add_dictionary_request_rejects_invalid_code() {
        let req = AddDictionaryRequest {
            code: "../en".to_string(),
            source_url: "https://example.com/dict".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn add_dictionary_request_rejects_non_http_url() {
        let req = AddDictionaryRequest {
            code: "fr_FR".to_string(),
            source_url: "ftp://example.com/dict".to_string(),
        };
        assert!(req.validate().is_err());
    }
}
