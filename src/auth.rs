//! Axum middleware: `X-API-Key` extraction/validation, admin-role gate, and
//! per-IP auth-failure rate limiting.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::AppError;
use crate::handlers::AppState;
use crate::store::{KeyRecord, Role};

/// Per-IP sliding-window limiter on authentication *failures* (not all
/// traffic). Successful requests never touch this.
pub struct RateLimiter {
    state: Mutex<HashMap<IpAddr, FailureWindow>>,
    max_failures: u32,
    window: Duration,
    cooldown: Duration,
}

struct FailureWindow {
    failures: Vec<Instant>,
    cooldown_until: Option<Instant>,
}

impl RateLimiter {
    pub fn new(max_failures: u32, window: Duration, cooldown: Duration) -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            max_failures,
            window,
            cooldown,
        }
    }

    /// `Err(remaining)` if this IP is currently in a cooldown.
    fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let state = self.state.lock().unwrap();
        if let Some(w) = state.get(&ip) {
            if let Some(until) = w.cooldown_until {
                let now = Instant::now();
                if now < until {
                    return Err(until - now);
                }
            }
        }
        Ok(())
    }

    /// Records a failed auth attempt; may start a new cooldown for this IP.
    fn record_failure(&self, ip: IpAddr) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        let window = self.window;
        let entry = state.entry(ip).or_insert_with(|| FailureWindow {
            failures: Vec::new(),
            cooldown_until: None,
        });
        entry.failures.retain(|t| now.duration_since(*t) < window);
        entry.failures.push(now);
        if entry.failures.len() as u32 >= self.max_failures {
            entry.cooldown_until = Some(now + self.cooldown);
            entry.failures.clear();
        }
    }
}

fn client_ip(connect_info: &Option<ConnectInfo<SocketAddr>>) -> IpAddr {
    connect_info
        .map(|ConnectInfo(addr)| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Requires a valid, active `X-API-Key`. On success, inserts the resolved
/// [`KeyRecord`] into the request's extensions for downstream handlers/layers
/// (e.g. [`require_admin`]) to read.
///
/// `ConnectInfo` is optional so this also works under `Router::oneshot` in
/// tests, which don't go through `into_make_service_with_connect_info` — it
/// just falls back to `0.0.0.0` for rate-limiting purposes in that case.
pub async fn require_active_key(
    State(state): State<Arc<AppState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    mut request: Request,
    next: Next,
) -> Response {
    let ip = client_ip(&connect_info);

    if let Err(remaining) = state.rate_limiter.check(ip) {
        return AppError::RateLimited {
            retry_after_secs: remaining.as_secs().max(1),
        }
        .into_response();
    }

    let raw_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let Some(raw_key) = raw_key else {
        state.rate_limiter.record_failure(ip);
        return AppError::Unauthorized.into_response();
    };

    let Some(record) = state.store.authenticate(&raw_key) else {
        state.rate_limiter.record_failure(ip);
        return AppError::Unauthorized.into_response();
    };

    state.store.touch_last_used(&record.id);
    request.extensions_mut().insert(record);
    next.run(request).await
}

/// Requires the key resolved by [`require_active_key`] (must run after it in
/// the layer chain) to have the `admin` role. `platform` keys are also
/// rejected here — they have no `tenant_id`, so they can't be used against
/// tenant-scoped endpoints like `/api-keys*` (`platform` gets its own
/// dedicated routes and gate, [`require_platform_key`]).
pub async fn require_admin(request: Request, next: Next) -> Response {
    let record = request
        .extensions()
        .get::<KeyRecord>()
        .expect("require_admin must be layered after require_active_key");
    if record.role != Role::Admin {
        return AppError::Forbidden.into_response();
    }
    next.run(request).await
}

/// Gate for `/tenants*`: requires a `platform`-role key, and rejects (403)
/// any request carrying an `Origin` header at all (F43a) — `platform` keys
/// are for server-to-server use only (the billing app's backend), never
/// callable from browser JS. This subsumes key resolution rather than
/// composing with [`require_active_key`]: the `Origin` rejection applies
/// unconditionally, before even looking at whether the key is valid, so a
/// leaked platform key can't be probed for validity from a browser context.
pub async fn require_platform_key(
    State(state): State<Arc<AppState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return AppError::Forbidden.into_response();
    }

    let ip = client_ip(&connect_info);

    if let Err(remaining) = state.rate_limiter.check(ip) {
        return AppError::RateLimited {
            retry_after_secs: remaining.as_secs().max(1),
        }
        .into_response();
    }

    let raw_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let Some(raw_key) = raw_key else {
        state.rate_limiter.record_failure(ip);
        return AppError::Unauthorized.into_response();
    };

    let Some(record) = state.store.authenticate(&raw_key) else {
        state.rate_limiter.record_failure(ip);
        return AppError::Unauthorized.into_response();
    };

    if record.role != Role::Platform {
        return AppError::Forbidden.into_response();
    }

    state.store.touch_last_used(&record.id);
    request.extensions_mut().insert(record);
    next.run(request).await
}

/// If the request carries an `Origin` header, it must be registered to the
/// calling key's own tenant, or the request is rejected (403) — a real
/// server-side check, independent of the `Access-Control-Allow-Origin`
/// response header `middleware::cors_layer` sets. Requests without an
/// `Origin` header (server-to-server clients) skip this entirely. Must run
/// after [`require_active_key`] and only on tenant-scoped routes.
pub async fn require_origin_binding(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let Ok(origin_str) = origin.to_str() else {
            return AppError::Forbidden.into_response();
        };
        let record = request
            .extensions()
            .get::<KeyRecord>()
            .expect("require_origin_binding must be layered after require_active_key");
        let tenant_id = record
            .tenant_id
            .as_deref()
            .expect("require_origin_binding runs only on tenant-scoped routes");
        if !state.store.tenant_owns_origin(tenant_id, origin_str) {
            return AppError::Forbidden.into_response();
        }
    }
    next.run(request).await
}

/// Rejects (403) if the calling key's tenant is suspended. Must run after
/// [`require_active_key`] and only on tenant-scoped routes.
pub async fn require_active_tenant(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let record = request
        .extensions()
        .get::<KeyRecord>()
        .expect("require_active_tenant must be layered after require_active_key");
    let tenant_id = record
        .tenant_id
        .as_deref()
        .expect("require_active_tenant runs only on tenant-scoped routes");
    let suspended = state
        .store
        .get_tenant(tenant_id)
        .map(|t| t.suspended_at.is_some())
        .unwrap_or(false);
    if suspended {
        return AppError::Forbidden.into_response();
    }
    next.run(request).await
}

/// Which slice of the usage rollup a `/usage/*` caller may read (§26.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageScope {
    /// Cross-tenant aggregates, for the billing app's platform key.
    Platform,
    /// Restricted to one tenant. Handlers pass this straight through to the
    /// store's `tenant_id` filter, which is what makes F61 structural.
    Tenant(String),
}

/// Gate for `/usage/*`. The existing groups can't express this one: F60 admits
/// both `platform` (no tenant) and `admin` (tenant-scoped), while
/// [`require_active_tenant`] assumes a tenant exists and
/// [`require_platform_key`] excludes admins. Composes the same checks those
/// layers make rather than reimplementing them. Must run after
/// [`require_active_key`].
pub async fn require_usage_scope(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let record = request
        .extensions()
        .get::<KeyRecord>()
        .expect("require_usage_scope must be layered after require_active_key")
        .clone();

    let scope = match record.role {
        // Usage is not self-service telemetry for an ordinary integration key.
        Role::Standard => return AppError::Forbidden.into_response(),
        Role::Platform => {
            // Same server-to-server rule as F43a: a platform key has no
            // tenant and therefore no registered origins to bind against.
            if request.headers().contains_key(header::ORIGIN) {
                return AppError::Forbidden.into_response();
            }
            UsageScope::Platform
        }
        Role::Admin => {
            let Some(tenant_id) = record.tenant_id.clone() else {
                return AppError::Forbidden.into_response();
            };
            let Some(tenant) = state.store.get_tenant(&tenant_id) else {
                return AppError::Forbidden.into_response();
            };
            if tenant.suspended_at.is_some() {
                return AppError::Forbidden.into_response();
            }
            if let Some(origin) = request.headers().get(header::ORIGIN) {
                let Ok(origin_str) = origin.to_str() else {
                    return AppError::Forbidden.into_response();
                };
                if !state.store.tenant_owns_origin(&tenant_id, origin_str) {
                    return AppError::Forbidden.into_response();
                }
            }
            UsageScope::Tenant(tenant_id)
        }
    };

    request.extensions_mut().insert(scope);
    next.run(request).await
}

/// Enforces the calling tenant's request quota on `/spellcheck*` only.
/// Rejects (429, distinct from [`AppError::RateLimited`]) if the tenant is
/// at or over `quota_limit`; otherwise consumes one unit of quota. Must run
/// after [`require_active_key`], and should run after [`require_active_tenant`]
/// so a suspended tenant's blocked requests don't also burn quota.
pub async fn require_quota(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let record = request
        .extensions()
        .get::<KeyRecord>()
        .expect("require_quota must be layered after require_active_key");
    let tenant_id = record
        .tenant_id
        .as_deref()
        .expect("require_quota runs only on tenant-scoped routes");

    if !state.store.try_consume_quota(tenant_id) {
        return AppError::QuotaExceeded.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_until_threshold() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60), Duration::from_secs(60));
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert!(limiter.check(ip).is_ok());
        limiter.record_failure(ip);
        assert!(limiter.check(ip).is_ok());
        limiter.record_failure(ip);
        assert!(limiter.check(ip).is_ok());
        limiter.record_failure(ip);

        assert!(
            limiter.check(ip).is_err(),
            "should be in cooldown at threshold"
        );
    }

    #[test]
    fn rate_limiter_tracks_ips_independently() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60), Duration::from_secs(60));
        let ip_a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        limiter.record_failure(ip_a);
        assert!(limiter.check(ip_a).is_err());
        assert!(limiter.check(ip_b).is_ok());
    }
}
