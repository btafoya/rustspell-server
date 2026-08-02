//! Pluggable persistence layer (SQLite or PostgreSQL) for API keys, tenants, and
//! registered CORS origins, plus in-memory read caches for the auth/CORS hot path.
//!
//! Backed by `sqlx::Any` so the same query surface works against either backend
//! (`?` placeholders, rewritten internally for Postgres) — see `DESIGN.md` §20.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::config::Config;
use crate::usage::{DailyCounters, DailyKey, LatencyKey, UsageDailyRow, UsageLatencyRow};

/// A key's role. `Platform` keys manage tenants and have no `tenant_id`;
/// `Admin`/`Standard` keys always belong to exactly one tenant.
///
/// Reused directly as the HTTP-facing type in `models.rs` rather than
/// duplicating it — this is also a persistence value (the `role` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Platform,
    Admin,
    Standard,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Platform => "platform",
            Role::Admin => "admin",
            Role::Standard => "standard",
        }
    }

    fn parse(s: &str) -> Option<Role> {
        match s {
            "platform" => Some(Role::Platform),
            "admin" => Some(Role::Admin),
            "standard" => Some(Role::Standard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyRecord {
    pub id: String,
    pub tenant_id: Option<String>,
    pub label: String,
    pub role: Role,
    pub key_hash: String,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
}

impl KeyRecord {
    fn is_active(&self, now: u64) -> bool {
        self.revoked_at.is_none() && self.expires_at.map(|exp| exp > now).unwrap_or(true)
    }
}

#[derive(Debug, Clone)]
pub struct TenantInfo {
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

#[derive(Debug, Clone)]
pub struct OriginInfo {
    pub id: String,
    pub tenant_id: String,
    pub origin: String,
    pub created_at: u64,
}

/// A registered dictionary locale and its source URL template.
#[derive(Debug, Clone)]
pub struct DictionaryInfo {
    pub code: String,
    pub source_url: String,
    pub created_at: u64,
}

/// A freshly generated key: the raw value is only ever available here, once.
#[derive(Debug)]
pub struct CreatedApiKey {
    pub record: KeyRecord,
    pub raw_key: String,
}

pub struct Store {
    pool: AnyPool,
    keys: RwLock<HashMap<String, KeyRecord>>, // by key_hash
    tenants: RwLock<HashMap<String, TenantInfo>>, // by tenant id
    origins_any: RwLock<HashSet<String>>,
    /// tenant id -> origin string -> full record, so `list_origins` needs no I/O.
    origins_by_tenant: RwLock<HashMap<String, HashMap<String, OriginInfo>>>,
    /// code -> full record, so `list_dictionaries` needs no I/O.
    dictionaries: RwLock<HashMap<String, DictionaryInfo>>,
}

impl Store {
    /// Connects, initializes the schema, loads the caches, and — if no active
    /// `platform` key exists — bootstraps one (F22).
    pub async fn open(config: &Config) -> anyhow::Result<(Self, Option<CreatedApiKey>)> {
        let store = Self::open_internal(config).await?;

        let bootstrap = if !store.has_active_platform_key() {
            Some(
                store
                    .create_key_internal(None, "bootstrap".to_string(), Role::Platform, None)
                    .await?,
            )
        } else {
            None
        };

        Ok((store, bootstrap))
    }

    /// Same as [`Store::open`] minus bootstrap-key creation. Intended for
    /// offline administrative commands that must not warm dictionaries or
    /// start server runtime.
    pub async fn open_for_cli(config: &Config) -> anyhow::Result<Self> {
        Self::open_internal(config).await
    }

    async fn open_internal(config: &Config) -> anyhow::Result<Self> {
        sqlx::any::install_default_drivers();

        let url = match &config.db_url {
            Some(pg_url) => pg_url.clone(),
            None => {
                if let Some(parent) = config.db_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                format!("sqlite://{}?mode=rwc", config.db_path.display())
            }
        };
        // Decide pooling/pragmas from the actual URL scheme rather than which
        // Config field it came from, so a test-supplied `sqlite::memory:` in
        // `db_url` still gets single-connection + WAL treatment.
        let is_sqlite = !url.starts_with("postgres");

        let pool = AnyPoolOptions::new()
            .max_connections(if is_sqlite { 1 } else { 10 })
            .connect(&url)
            .await?;

        if is_sqlite {
            sqlx::query("PRAGMA journal_mode=WAL;")
                .execute(&pool)
                .await?;
        }

        init_schema(&pool).await?;

        let store = Store {
            pool,
            keys: RwLock::new(HashMap::new()),
            tenants: RwLock::new(HashMap::new()),
            origins_any: RwLock::new(HashSet::new()),
            origins_by_tenant: RwLock::new(HashMap::new()),
            dictionaries: RwLock::new(HashMap::new()),
        };

        store.reload_caches().await?;

        Ok(store)
    }

    async fn reload_caches(&self) -> anyhow::Result<()> {
        let key_rows = sqlx::query(
            "SELECT id, COALESCE(tenant_id, '') AS tenant_id, label, role, key_hash, created_at, \
             COALESCE(expires_at, -1) AS expires_at, COALESCE(last_used_at, -1) AS last_used_at, \
             COALESCE(revoked_at, -1) AS revoked_at FROM api_keys",
        )
        .fetch_all(&self.pool)
        .await?;
        {
            let mut keys = self.keys.write().unwrap();
            for row in key_rows {
                let record = key_record_from_row(&row)?;
                keys.insert(record.key_hash.clone(), record);
            }
        }

        let tenant_rows = sqlx::query(
            "SELECT id, name, language, quota_limit, request_count, \
             COALESCE(period_start, -1) AS period_start, COALESCE(period_end, -1) AS period_end, \
             COALESCE(suspended_at, -1) AS suspended_at, created_at FROM tenants",
        )
        .fetch_all(&self.pool)
        .await?;
        {
            let mut tenants = self.tenants.write().unwrap();
            for row in tenant_rows {
                let info = tenant_info_from_row(&row)?;
                tenants.insert(info.id.clone(), info);
            }
        }

        let origin_rows =
            sqlx::query("SELECT id, tenant_id, origin, created_at FROM tenant_origins")
                .fetch_all(&self.pool)
                .await?;
        {
            let mut origins_any = self.origins_any.write().unwrap();
            let mut origins_by_tenant = self.origins_by_tenant.write().unwrap();
            for row in origin_rows {
                let info = origin_info_from_row(&row)?;
                origins_any.insert(info.origin.clone());
                origins_by_tenant
                    .entry(info.tenant_id.clone())
                    .or_default()
                    .insert(info.origin.clone(), info);
            }
        }

        let dictionary_rows = sqlx::query("SELECT code, source_url, created_at FROM dictionaries")
            .fetch_all(&self.pool)
            .await?;
        {
            let mut dictionaries = self.dictionaries.write().unwrap();
            for row in dictionary_rows {
                let info = dictionary_info_from_row(&row)?;
                dictionaries.insert(info.code.clone(), info);
            }
        }

        Ok(())
    }

    fn has_active_platform_key(&self) -> bool {
        let now = now();
        self.keys
            .read()
            .unwrap()
            .values()
            .any(|k| k.role == Role::Platform && k.is_active(now))
    }

    // ---- Keys ----------------------------------------------------------

    pub async fn create_key(
        &self,
        tenant_id: &str,
        label: String,
        role: Role,
        expires_at: Option<u64>,
    ) -> anyhow::Result<CreatedApiKey> {
        self.create_key_internal(Some(tenant_id.to_string()), label, role, expires_at)
            .await
    }

    async fn create_key_internal(
        &self,
        tenant_id: Option<String>,
        label: String,
        role: Role,
        expires_at: Option<u64>,
    ) -> anyhow::Result<CreatedApiKey> {
        let id = new_id();
        let raw_key = generate_raw_key();
        let key_hash = hash_key(&raw_key);
        let created_at = now();

        sqlx::query(
            "INSERT INTO api_keys (id, tenant_id, label, role, key_hash, created_at, expires_at, last_used_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(&tenant_id)
        .bind(&label)
        .bind(role.as_str())
        .bind(&key_hash)
        .bind(created_at as i64)
        .bind(expires_at.map(|v| v as i64))
        .execute(&self.pool)
        .await?;

        let record = KeyRecord {
            id,
            tenant_id,
            label,
            role,
            key_hash: key_hash.clone(),
            created_at,
            expires_at,
            last_used_at: None,
            revoked_at: None,
        };
        self.keys
            .write()
            .unwrap()
            .insert(key_hash.clone(), record.clone());

        Ok(CreatedApiKey { record, raw_key })
    }

    /// Keys belonging to `tenant_id` (never `platform` keys, which have none).
    pub fn list_keys(&self, tenant_id: &str) -> Vec<KeyRecord> {
        self.keys
            .read()
            .unwrap()
            .values()
            .filter(|k| k.tenant_id.as_deref() == Some(tenant_id))
            .cloned()
            .collect()
    }

    /// `Ok(false)` if `id` doesn't exist or belongs to a different tenant.
    pub async fn revoke_key(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool> {
        let key_hash = {
            let keys = self.keys.read().unwrap();
            match keys
                .values()
                .find(|k| k.id == id && k.tenant_id.as_deref() == Some(tenant_id))
            {
                Some(k) => k.key_hash.clone(),
                None => return Ok(false),
            }
        };

        let revoked_at = now() as i64;
        sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(revoked_at)
            .bind(id)
            .execute(&self.pool)
            .await?;

        if let Some(record) = self.keys.write().unwrap().get_mut(&key_hash) {
            // Idempotent: an already-revoked key keeps its original timestamp —
            // the UPDATE above is a no-op for it (WHERE revoked_at IS NULL), so
            // the cache must not overwrite what's actually in the database.
            if record.revoked_at.is_none() {
                record.revoked_at = Some(revoked_at as u64);
            }
        }
        Ok(true)
    }

    /// `Ok(None)` if `id` doesn't exist or belongs to a different tenant.
    pub async fn rotate_key(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> anyhow::Result<Option<CreatedApiKey>> {
        let old_hash = {
            let keys = self.keys.read().unwrap();
            match keys
                .values()
                .find(|k| k.id == id && k.tenant_id.as_deref() == Some(tenant_id))
            {
                Some(k) => k.key_hash.clone(),
                None => return Ok(None),
            }
        };

        self.rotate_key_by_hash(&old_hash, id).await.map(Some)
    }

    /// Rotate the bootstrap `platform` key (no tenant), or create one if none
    /// exists. Fails if more than one active bootstrap key is present so the
    /// operation is unambiguous.
    pub async fn reset_bootstrap_platform_key(&self) -> anyhow::Result<CreatedApiKey> {
        let now = now();
        let bootstrap_records: Vec<KeyRecord> = {
            let keys = self.keys.read().unwrap();
            keys.values()
                .filter(|k| {
                    k.role == Role::Platform
                        && k.label == "bootstrap"
                        && k.tenant_id.is_none()
                        && k.is_active(now)
                })
                .cloned()
                .collect()
        };

        match bootstrap_records.len() {
            0 => {
                self.create_key_internal(None, "bootstrap".to_string(), Role::Platform, None)
                    .await
            }
            1 => {
                let record = &bootstrap_records[0];
                self.rotate_key_by_hash(&record.key_hash, &record.id).await
            }
            _ => anyhow::bail!(
                "found {} active platform keys labeled 'bootstrap'; reset is ambiguous",
                bootstrap_records.len()
            ),
        }
    }

    async fn rotate_key_by_hash(&self, old_hash: &str, id: &str) -> anyhow::Result<CreatedApiKey> {
        let raw_key = generate_raw_key();
        let new_hash = hash_key(&raw_key);

        sqlx::query("UPDATE api_keys SET key_hash = ?, last_used_at = NULL WHERE id = ?")
            .bind(&new_hash)
            .bind(id)
            .execute(&self.pool)
            .await?;

        let mut keys = self.keys.write().unwrap();
        let mut record = keys.remove(old_hash).expect("checked above");
        record.key_hash = new_hash.clone();
        record.last_used_at = None;
        keys.insert(new_hash, record.clone());

        Ok(CreatedApiKey { record, raw_key })
    }

    /// Hot-path lookup: hash the raw key, check the cache, verify active. If the
    /// cache misses, fall back to a database query so rotations performed by an
    /// offline CLI process (or any other out-of-process change) are honored by
    /// a running server without requiring a restart.
    pub async fn authenticate(&self, raw_key: &str) -> Option<KeyRecord> {
        let hash = hash_key(raw_key);
        let now = now();

        {
            let keys = self.keys.read().unwrap();
            if let Some(record) = keys.get(&hash).filter(|k| k.is_active(now)) {
                return Some(record.clone());
            }
        }

        let record = self.load_key_by_hash(&hash).await.ok()??;
        if !record.is_active(now) {
            return None;
        }

        let mut keys = self.keys.write().unwrap();
        // Rotations performed by another process change the key_hash for an
        // existing id. Remove any stale hash for this id so the old value stops
        // authenticating as soon as the new value is used.
        keys.retain(|_, k| k.id != record.id);
        keys.insert(hash, record.clone());
        Some(record)
    }

    async fn load_key_by_hash(&self, key_hash: &str) -> anyhow::Result<Option<KeyRecord>> {
        let row = sqlx::query(
            "SELECT id, COALESCE(tenant_id, '') AS tenant_id, label, role, key_hash, created_at, \
             COALESCE(expires_at, -1) AS expires_at, COALESCE(last_used_at, -1) AS last_used_at, \
             COALESCE(revoked_at, -1) AS revoked_at FROM api_keys WHERE key_hash = ?",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(key_record_from_row(&row)?)),
            None => Ok(None),
        }
    }

    /// Fire-and-forget: updates the cache synchronously (so an immediately
    /// following `list_keys` reflects it) and spawns a background write.
    /// Errors are logged, never surfaced — this must not slow down or fail
    /// the request that triggered it. See `DESIGN.md` §16's write-volume note.
    pub fn touch_last_used(&self, id: &str) {
        let now_ts = now();

        let key_hash = self
            .keys
            .read()
            .unwrap()
            .values()
            .find(|k| k.id == id)
            .map(|k| k.key_hash.clone());
        if let Some(hash) = key_hash {
            if let Some(record) = self.keys.write().unwrap().get_mut(&hash) {
                record.last_used_at = Some(now_ts);
            }
        }

        let pool = self.pool.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
                .bind(now_ts as i64)
                .bind(&id)
                .execute(&pool)
                .await
            {
                tracing::warn!("failed to persist last_used_at for key {id}: {e}");
            }
        });
    }

    // ---- Tenants ---------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn create_tenant(
        &self,
        name: String,
        language: Option<String>,
        quota_limit: Option<u64>,
        period_start: Option<u64>,
        period_end: Option<u64>,
    ) -> anyhow::Result<(TenantInfo, CreatedApiKey)> {
        let id = new_id();
        let language = language.unwrap_or_else(|| crate::config::DEFAULT_LANGUAGE.to_string());
        let quota_limit = quota_limit.unwrap_or(0);
        let created_at = now();

        sqlx::query(
            "INSERT INTO tenants (id, name, language, quota_limit, request_count, period_start, period_end, suspended_at, created_at) VALUES (?, ?, ?, ?, 0, ?, ?, NULL, ?)",
        )
        .bind(&id)
        .bind(&name)
        .bind(&language)
        .bind(quota_limit as i64)
        .bind(period_start.map(|v| v as i64))
        .bind(period_end.map(|v| v as i64))
        .bind(created_at as i64)
        .execute(&self.pool)
        .await?;

        let info = TenantInfo {
            id: id.clone(),
            name,
            language,
            quota_limit,
            request_count: 0,
            period_start,
            period_end,
            suspended_at: None,
            created_at,
        };
        self.tenants
            .write()
            .unwrap()
            .insert(id.clone(), info.clone());

        let admin_key = self
            .create_key_internal(Some(id), "default".to_string(), Role::Admin, None)
            .await?;

        Ok((info, admin_key))
    }

    pub fn list_tenants(&self) -> Vec<TenantInfo> {
        self.tenants.read().unwrap().values().cloned().collect()
    }

    pub fn get_tenant(&self, id: &str) -> Option<TenantInfo> {
        self.tenants.read().unwrap().get(id).cloned()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_tenant(
        &self,
        id: &str,
        name: Option<String>,
        language: Option<String>,
        quota_limit: Option<u64>,
        request_count: Option<u64>,
        period_start: Option<Option<u64>>,
        period_end: Option<Option<u64>>,
    ) -> anyhow::Result<Option<TenantInfo>> {
        if !self.tenants.read().unwrap().contains_key(id) {
            return Ok(None);
        }

        let snapshot = {
            let mut tenants = self.tenants.write().unwrap();
            let info = tenants.get_mut(id).expect("checked above");
            if let Some(name) = &name {
                info.name = name.clone();
            }
            if let Some(language) = &language {
                info.language = language.clone();
            }
            if let Some(quota_limit) = quota_limit {
                info.quota_limit = quota_limit;
            }
            // request_count reset (e.g. on billing period rollover) is the
            // only way it ever changes outside `try_consume_quota` — F46.
            if let Some(request_count) = request_count {
                info.request_count = request_count;
            }
            if let Some(period_start) = period_start {
                info.period_start = period_start;
            }
            if let Some(period_end) = period_end {
                info.period_end = period_end;
            }
            info.clone()
        };

        sqlx::query(
            "UPDATE tenants SET name = ?, language = ?, quota_limit = ?, request_count = ?, period_start = ?, period_end = ? WHERE id = ?",
        )
        .bind(&snapshot.name)
        .bind(&snapshot.language)
        .bind(snapshot.quota_limit as i64)
        .bind(snapshot.request_count as i64)
        .bind(snapshot.period_start.map(|v| v as i64))
        .bind(snapshot.period_end.map(|v| v as i64))
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Some(snapshot))
    }

    /// Hot-path quota check: `true` and increments `request_count` if the
    /// tenant is under `quota_limit` (`0` == unlimited); `false` otherwise.
    /// The read-check-increment happens inside one write-lock critical
    /// section — unlike `touch_last_used`'s timestamp, this is the actual
    /// billing enforcement boundary, so two concurrent requests must not
    /// both read "one under the limit" and both be admitted.
    pub fn try_consume_quota(&self, tenant_id: &str) -> bool {
        let new_count = {
            let mut tenants = self.tenants.write().unwrap();
            let Some(tenant) = tenants.get_mut(tenant_id) else {
                return false; // no tenant for an authenticated key should not happen; fail closed
            };
            if tenant.quota_limit > 0 && tenant.request_count >= tenant.quota_limit {
                return false;
            }
            tenant.request_count += 1;
            tenant.request_count
        };

        // Fire-and-forget persistence, same tradeoff as `touch_last_used`: a
        // crash between increment and flush undercounts usage from the DB's
        // perspective (a little free quota), never overcounts into a false 429.
        let pool = self.pool.clone();
        let tenant_id = tenant_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query("UPDATE tenants SET request_count = ? WHERE id = ?")
                .bind(new_count as i64)
                .bind(&tenant_id)
                .execute(&pool)
                .await
            {
                tracing::warn!("failed to persist request_count for tenant {tenant_id}: {e}");
            }
        });

        true
    }

    pub async fn set_suspended(&self, id: &str, suspended: bool) -> anyhow::Result<bool> {
        if !self.tenants.read().unwrap().contains_key(id) {
            return Ok(false);
        }
        let suspended_at = if suspended { Some(now()) } else { None };

        sqlx::query("UPDATE tenants SET suspended_at = ? WHERE id = ?")
            .bind(suspended_at.map(|v| v as i64))
            .bind(id)
            .execute(&self.pool)
            .await?;

        if let Some(info) = self.tenants.write().unwrap().get_mut(id) {
            info.suspended_at = suspended_at;
        }
        Ok(true)
    }

    // ---- Origins -----------------------------------------------------------

    pub async fn register_origin(
        &self,
        tenant_id: &str,
        origin: String,
    ) -> anyhow::Result<OriginInfo> {
        let id = new_id();
        let created_at = now();

        sqlx::query(
            "INSERT INTO tenant_origins (id, tenant_id, origin, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(&origin)
        .bind(created_at as i64)
        .execute(&self.pool)
        .await?;

        let info = OriginInfo {
            id,
            tenant_id: tenant_id.to_string(),
            origin: origin.clone(),
            created_at,
        };

        self.origins_any.write().unwrap().insert(origin.clone());
        self.origins_by_tenant
            .write()
            .unwrap()
            .entry(tenant_id.to_string())
            .or_default()
            .insert(origin, info.clone());

        Ok(info)
    }

    /// Sync, cache-only — no I/O.
    pub fn list_origins(&self, tenant_id: &str) -> Vec<OriginInfo> {
        self.origins_by_tenant
            .read()
            .unwrap()
            .get(tenant_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// `Ok(false)` if `id` doesn't exist or belongs to a different tenant.
    pub async fn revoke_origin(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool> {
        let row: Option<AnyRow> =
            sqlx::query("SELECT origin FROM tenant_origins WHERE id = ? AND tenant_id = ?")
                .bind(id)
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(row) = row else { return Ok(false) };
        let origin: String = row.try_get("origin")?;

        sqlx::query("DELETE FROM tenant_origins WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        if let Some(map) = self.origins_by_tenant.write().unwrap().get_mut(tenant_id) {
            map.remove(&origin);
        }
        // Only drop from the global set if no tenant owns it anymore.
        let still_owned = self
            .origins_by_tenant
            .read()
            .unwrap()
            .values()
            .any(|map| map.contains_key(&origin));
        if !still_owned {
            self.origins_any.write().unwrap().remove(&origin);
        }

        Ok(true)
    }

    /// Sync, in-memory only — safe to call from the CORS predicate.
    pub fn is_registered_origin(&self, origin: &str) -> bool {
        self.origins_any.read().unwrap().contains(origin)
    }

    /// Sync, in-memory only — safe to call from auth middleware.
    pub fn tenant_owns_origin(&self, tenant_id: &str, origin: &str) -> bool {
        self.origins_by_tenant
            .read()
            .unwrap()
            .get(tenant_id)
            .map(|map| map.contains_key(origin))
            .unwrap_or(false)
    }

    // ---- Dictionaries ----------------------------------------------------

    /// Persist a new dictionary locale and its source URL, or update the URL
    /// if the locale is already registered. Returns the actual stored record
    /// (preserving the original `created_at`).
    pub async fn register_dictionary(
        &self,
        code: String,
        source_url: String,
    ) -> anyhow::Result<DictionaryInfo> {
        let created_at = now();

        sqlx::query(
            "INSERT INTO dictionaries (code, source_url, created_at) VALUES (?, ?, ?) \
             ON CONFLICT (code) DO UPDATE SET source_url = excluded.source_url",
        )
        .bind(&code)
        .bind(&source_url)
        .bind(created_at as i64)
        .execute(&self.pool)
        .await?;

        let row =
            sqlx::query("SELECT code, source_url, created_at FROM dictionaries WHERE code = ?")
                .bind(&code)
                .fetch_one(&self.pool)
                .await?;
        let info = dictionary_info_from_row(&row)?;

        self.dictionaries
            .write()
            .unwrap()
            .insert(code, info.clone());

        Ok(info)
    }

    /// Sync, cache-only — no I/O.
    pub fn list_dictionaries(&self) -> Vec<DictionaryInfo> {
        self.dictionaries
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// Sync, cache-only — no I/O.
    pub fn get_dictionary(&self, code: &str) -> Option<DictionaryInfo> {
        self.dictionaries.read().unwrap().get(code).cloned()
    }

    // ---- Usage rollup (§26) --------------------------------------------

    /// Applies a drained [`UsageRecorder`](crate::usage::UsageRecorder) batch.
    /// The `ON CONFLICT … DO UPDATE` form accumulates rather than overwrites,
    /// which is what makes repeated flushes for the same day additive, and is
    /// spelled identically on SQLite and PostgreSQL (NF14).
    pub async fn flush_usage(
        &self,
        daily: Vec<(DailyKey, DailyCounters)>,
        latency: Vec<(LatencyKey, i64)>,
    ) -> anyhow::Result<()> {
        if daily.is_empty() && latency.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for (key, counters) in daily {
            sqlx::query(
                "INSERT INTO usage_daily (day, tenant_id, language, status, error_slug, request_count, latency_sum_us) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (day, tenant_id, language, status, error_slug) DO UPDATE SET \
                 request_count = usage_daily.request_count + excluded.request_count, \
                 latency_sum_us = usage_daily.latency_sum_us + excluded.latency_sum_us",
            )
            .bind(&key.day)
            .bind(&key.tenant_id)
            .bind(&key.language)
            .bind(key.status)
            .bind(&key.error_slug)
            .bind(counters.request_count)
            .bind(counters.latency_sum_us)
            .execute(&mut *tx)
            .await?;
        }

        for (key, count) in latency {
            sqlx::query(
                "INSERT INTO usage_latency (day, tenant_id, bucket_le_ms, request_count) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT (day, tenant_id, bucket_le_ms) DO UPDATE SET \
                 request_count = usage_latency.request_count + excluded.request_count",
            )
            .bind(&key.day)
            .bind(&key.tenant_id)
            .bind(key.bucket_le_ms)
            .bind(count)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// `tenant_id = None` is platform scope (no filter); `Some(id)` restricts
    /// to one tenant, which is what makes F61 structural — the percentage
    /// denominator is computed from whatever this returns.
    ///
    /// `SUM` is cast to `BIGINT` because PostgreSQL returns `NUMERIC` for
    /// `SUM(bigint)`, which `sqlx::Any` cannot decode as `i64`.
    pub async fn query_usage_daily(
        &self,
        tenant_id: Option<&str>,
        start: &str,
        end: &str,
    ) -> anyhow::Result<Vec<UsageDailyRow>> {
        let sql = "SELECT day, language, status, error_slug, \
                   CAST(SUM(request_count) AS BIGINT) AS request_count, \
                   CAST(SUM(latency_sum_us) AS BIGINT) AS latency_sum_us \
                   FROM usage_daily WHERE day >= ? AND day <= ?";
        let rows = match tenant_id {
            Some(id) => {
                sqlx::query(&format!(
                    "{sql} AND tenant_id = ? GROUP BY day, language, status, error_slug"
                ))
                .bind(start)
                .bind(end)
                .bind(id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!("{sql} GROUP BY day, language, status, error_slug"))
                    .bind(start)
                    .bind(end)
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        rows.iter()
            .map(|row| {
                Ok(UsageDailyRow {
                    day: row.try_get("day")?,
                    language: row.try_get("language")?,
                    status: row.try_get("status")?,
                    error_slug: row.try_get("error_slug")?,
                    request_count: row.try_get("request_count")?,
                    latency_sum_us: row.try_get("latency_sum_us")?,
                })
            })
            .collect()
    }

    pub async fn query_usage_latency(
        &self,
        tenant_id: Option<&str>,
        start: &str,
        end: &str,
    ) -> anyhow::Result<Vec<UsageLatencyRow>> {
        let sql = "SELECT day, bucket_le_ms, \
                   CAST(SUM(request_count) AS BIGINT) AS request_count \
                   FROM usage_latency WHERE day >= ? AND day <= ?";
        let rows = match tenant_id {
            Some(id) => {
                sqlx::query(&format!(
                    "{sql} AND tenant_id = ? GROUP BY day, bucket_le_ms"
                ))
                .bind(start)
                .bind(end)
                .bind(id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!("{sql} GROUP BY day, bucket_le_ms"))
                    .bind(start)
                    .bind(end)
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        rows.iter()
            .map(|row| {
                Ok(UsageLatencyRow {
                    day: row.try_get("day")?,
                    bucket_le_ms: row.try_get("bucket_le_ms")?,
                    request_count: row.try_get("request_count")?,
                })
            })
            .collect()
    }

    /// Drops rollup rows strictly older than `cutoff_day` (F51).
    pub async fn purge_usage_before(&self, cutoff_day: &str) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let daily = sqlx::query("DELETE FROM usage_daily WHERE day < ?")
            .bind(cutoff_day)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let latency = sqlx::query("DELETE FROM usage_latency WHERE day < ?")
            .bind(cutoff_day)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(daily + latency)
    }
}

async fn init_schema(pool: &AnyPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tenants (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            language      TEXT NOT NULL DEFAULT 'en_US',
            quota_limit   BIGINT NOT NULL DEFAULT 0,
            request_count BIGINT NOT NULL DEFAULT 0,
            period_start  BIGINT,
            period_end    BIGINT,
            suspended_at  BIGINT,
            created_at    BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tenant_origins (
            id         TEXT PRIMARY KEY,
            tenant_id  TEXT NOT NULL REFERENCES tenants(id),
            origin     TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            UNIQUE(tenant_id, origin)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tenant_origins_origin ON tenant_origins(origin)")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_tenant_origins_tenant ON tenant_origins(tenant_id)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS api_keys (
            id            TEXT PRIMARY KEY,
            tenant_id     TEXT REFERENCES tenants(id),
            label         TEXT NOT NULL,
            role          TEXT NOT NULL CHECK (role IN ('platform', 'admin', 'standard')),
            key_hash      TEXT NOT NULL UNIQUE,
            created_at    BIGINT NOT NULL,
            expires_at    BIGINT,
            last_used_at  BIGINT,
            revoked_at    BIGINT
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash)")
        .execute(pool)
        .await?;

    // Usage rollup (§26.1). Two tables rather than one: the full cross-product
    // would multiply row count by the latency ladder for no benefit, since no
    // endpoint needs latency split by both language and status. Every column
    // is NOT NULL, so none of them hit the `sqlx::Any` NULL-decode defect
    // documented below — `error_slug = ''` is what buys that on the one column
    // that would otherwise be nullable.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS dictionaries (
            code        TEXT PRIMARY KEY,
            source_url  TEXT NOT NULL,
            created_at  BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_daily (
            day            TEXT   NOT NULL,
            tenant_id      TEXT   NOT NULL,
            language       TEXT   NOT NULL,
            status         BIGINT NOT NULL,
            error_slug     TEXT   NOT NULL,
            request_count  BIGINT NOT NULL,
            latency_sum_us BIGINT NOT NULL,
            PRIMARY KEY (day, tenant_id, language, status, error_slug)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_latency (
            day           TEXT   NOT NULL,
            tenant_id     TEXT   NOT NULL,
            bucket_le_ms  BIGINT NOT NULL,
            request_count BIGINT NOT NULL,
            PRIMARY KEY (day, tenant_id, bucket_le_ms)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_usage_daily_day ON usage_daily(day)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_usage_latency_day ON usage_latency(day)")
        .execute(pool)
        .await?;

    Ok(())
}

/// `AnyRow::try_get::<Option<T>, _>` does not reliably decode a genuinely
/// NULL SQL value through `sqlx::Any`'s type-erased row — reproducible:
/// works for a row constructed in the same process, fails once the value
/// has actually round-tripped through a real database file and is read
/// back (see `reopen_file_backed_store_preserves_data`), and even a manual
/// `try_get_raw(..).is_null()` pre-check was unreliable. The queries in
/// `reload_caches` therefore `COALESCE` nullable columns to an
/// impossible-for-real-data sentinel (`-1` for timestamps, since every
/// timestamp this app writes comes from `u64`; `''` for `tenant_id`, since
/// tenant ids are UUIDs) and these functions translate the sentinel back to
/// `None` — decoding a concrete non-null `i64`/`String` is the
/// unquestionably-working path everywhere else in this file.
fn key_record_from_row(row: &AnyRow) -> anyhow::Result<KeyRecord> {
    let role_str: String = row.try_get("role")?;
    let role = Role::parse(&role_str)
        .ok_or_else(|| anyhow::anyhow!("invalid role in database: {role_str}"))?;
    let created_at: i64 = row.try_get("created_at")?;
    let expires_at: i64 = row.try_get("expires_at")?;
    let last_used_at: i64 = row.try_get("last_used_at")?;
    let revoked_at: i64 = row.try_get("revoked_at")?;
    let tenant_id: String = row.try_get("tenant_id")?;

    Ok(KeyRecord {
        id: row.try_get("id")?,
        tenant_id: (!tenant_id.is_empty()).then_some(tenant_id),
        label: row.try_get("label")?,
        role,
        key_hash: row.try_get("key_hash")?,
        created_at: created_at as u64,
        expires_at: (expires_at >= 0).then_some(expires_at as u64),
        last_used_at: (last_used_at >= 0).then_some(last_used_at as u64),
        revoked_at: (revoked_at >= 0).then_some(revoked_at as u64),
    })
}

fn tenant_info_from_row(row: &AnyRow) -> anyhow::Result<TenantInfo> {
    let quota_limit: i64 = row.try_get("quota_limit")?;
    let request_count: i64 = row.try_get("request_count")?;
    let period_start: i64 = row.try_get("period_start")?;
    let period_end: i64 = row.try_get("period_end")?;
    let suspended_at: i64 = row.try_get("suspended_at")?;
    let created_at: i64 = row.try_get("created_at")?;

    Ok(TenantInfo {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        language: row.try_get("language")?,
        quota_limit: quota_limit as u64,
        request_count: request_count as u64,
        period_start: (period_start >= 0).then_some(period_start as u64),
        period_end: (period_end >= 0).then_some(period_end as u64),
        suspended_at: (suspended_at >= 0).then_some(suspended_at as u64),
        created_at: created_at as u64,
    })
}

fn origin_info_from_row(row: &AnyRow) -> anyhow::Result<OriginInfo> {
    let created_at: i64 = row.try_get("created_at")?;
    Ok(OriginInfo {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        origin: row.try_get("origin")?,
        created_at: created_at as u64,
    })
}

fn dictionary_info_from_row(row: &AnyRow) -> anyhow::Result<DictionaryInfo> {
    let created_at: i64 = row.try_get("created_at")?;
    Ok(DictionaryInfo {
        code: row.try_get("code")?,
        source_url: row.try_get("source_url")?,
        created_at: created_at as u64,
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Raw value shown to the caller exactly once: "rsk_" + 2xUUIDv4 (no dashes).
fn generate_raw_key() -> String {
    format!("rsk_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash_key(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `Store` directly against an in-memory SQLite pool, bypassing
    /// `Store::open`'s file-path/`Config` handling (irrelevant to these tests).
    async fn open_test_store() -> (Store, Option<CreatedApiKey>) {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        init_schema(&pool).await.unwrap();

        let store = Store {
            pool,
            keys: RwLock::new(HashMap::new()),
            tenants: RwLock::new(HashMap::new()),
            origins_any: RwLock::new(HashSet::new()),
            origins_by_tenant: RwLock::new(HashMap::new()),
            dictionaries: RwLock::new(HashMap::new()),
        };
        store.reload_caches().await.unwrap();
        let bootstrap = if !store.has_active_platform_key() {
            Some(
                store
                    .create_key_internal(None, "bootstrap".to_string(), Role::Platform, None)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        (store, bootstrap)
    }

    #[tokio::test]
    async fn bootstrap_creates_one_platform_key_on_empty_store() {
        let (store, bootstrap) = open_test_store().await;
        let key = bootstrap.expect("empty store should bootstrap a platform key");
        assert_eq!(key.record.role, Role::Platform);
        assert!(key.record.tenant_id.is_none());
        assert!(store.authenticate(&key.raw_key).await.is_some());
    }

    #[tokio::test]
    async fn key_create_authenticate_revoke_round_trip() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin_key) = store
            .create_tenant("Acme".to_string(), None, None, None, None)
            .await
            .unwrap();

        let created = store
            .create_key(&tenant.id, "ci".to_string(), Role::Standard, None)
            .await
            .unwrap();
        assert!(store.authenticate(&created.raw_key).await.is_some());

        let revoked = store
            .revoke_key(&tenant.id, &created.record.id)
            .await
            .unwrap();
        assert!(revoked);
        assert!(store.authenticate(&created.raw_key).await.is_none());
    }

    #[tokio::test]
    async fn rotate_key_invalidates_old_value_and_keeps_identity() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin_key) = store
            .create_tenant("Acme".to_string(), None, None, None, None)
            .await
            .unwrap();
        let created = store
            .create_key(&tenant.id, "ci".to_string(), Role::Standard, None)
            .await
            .unwrap();

        let rotated = store
            .rotate_key(&tenant.id, &created.record.id)
            .await
            .unwrap()
            .expect("id belongs to this tenant");

        assert_ne!(rotated.raw_key, created.raw_key);
        assert_eq!(rotated.record.id, created.record.id);
        assert_eq!(rotated.record.label, created.record.label);
        assert_eq!(rotated.record.role, created.record.role);
        assert!(store.authenticate(&created.raw_key).await.is_none());
        assert!(store.authenticate(&rotated.raw_key).await.is_some());
    }

    #[tokio::test]
    async fn rotate_key_returns_none_for_other_tenants_key() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant_a, _) = store
            .create_tenant("A".to_string(), None, None, None, None)
            .await
            .unwrap();
        let (tenant_b, _) = store
            .create_tenant("B".to_string(), None, None, None, None)
            .await
            .unwrap();
        let key_b = store
            .create_key(&tenant_b.id, "b-key".to_string(), Role::Standard, None)
            .await
            .unwrap();

        let result = store
            .rotate_key(&tenant_a.id, &key_b.record.id)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn authenticate_rejects_expired_key() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin_key) = store
            .create_tenant("Acme".to_string(), None, None, None, None)
            .await
            .unwrap();
        // expires_at = 1 (1970-01-01T00:00:01Z) is always in the past.
        let created = store
            .create_key(&tenant.id, "expired".to_string(), Role::Standard, Some(1))
            .await
            .unwrap();

        assert!(store.authenticate(&created.raw_key).await.is_none());
    }

    #[tokio::test]
    async fn reopen_file_backed_store_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: dir.path().to_path_buf(),
            refresh_interval_hours: 24,
            dictionary_admin_cidrs: Vec::new(),
            trusted_proxies: Vec::new(),
            db_path: dir.path().join("test.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };

        let (tenant_id, raw_key) = {
            let (store, _bootstrap) = Store::open(&config).await.unwrap();
            let (tenant, _admin_key) = store
                .create_tenant("Acme".to_string(), None, None, None, None)
                .await
                .unwrap();
            let created = store
                .create_key(&tenant.id, "ci".to_string(), Role::Standard, None)
                .await
                .unwrap();
            (tenant.id, created.raw_key)
        };
        // `store` dropped here — simulates a process restart against the same file.

        let (store, bootstrap) = Store::open(&config).await.unwrap();
        assert!(
            bootstrap.is_none(),
            "existing active platform key should not re-bootstrap"
        );
        assert!(store.get_tenant(&tenant_id).is_some());
        assert!(store.authenticate(&raw_key).await.is_some());
    }

    /// Usage rows must survive a real restart, not just live in the cache —
    /// `:memory:` never exercises the reload-from-disk path.
    #[tokio::test]
    async fn reopen_file_backed_store_preserves_usage_rollup() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: dir.path().to_path_buf(),
            refresh_interval_hours: 24,
            dictionary_admin_cidrs: Vec::new(),
            trusted_proxies: Vec::new(),
            db_path: dir.path().join("usage.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };

        {
            let (store, _bootstrap) = Store::open(&config).await.unwrap();
            store
                .flush_usage(
                    vec![(
                        DailyKey {
                            day: "2026-07-30".to_string(),
                            tenant_id: "t1".to_string(),
                            language: "en_US".to_string(),
                            status: 200,
                            error_slug: String::new(),
                        },
                        DailyCounters {
                            request_count: 7,
                            latency_sum_us: 21_000,
                        },
                    )],
                    vec![(
                        LatencyKey {
                            day: "2026-07-30".to_string(),
                            tenant_id: "t1".to_string(),
                            bucket_le_ms: 5,
                        },
                        7,
                    )],
                )
                .await
                .unwrap();
        }
        // `store` dropped — simulates a container restart against the same file.

        let (store, _bootstrap) = Store::open(&config).await.unwrap();
        let rows = store
            .query_usage_daily(None, "2026-07-01", "2026-07-31")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].request_count, 7);
        assert_eq!(rows[0].latency_sum_us, 21_000);
        assert_eq!(rows[0].error_slug, "");

        let latency = store
            .query_usage_latency(None, "2026-07-01", "2026-07-31")
            .await
            .unwrap();
        assert_eq!(latency.len(), 1);
        assert_eq!(latency[0].request_count, 7);
    }

    /// The `ON CONFLICT … DO UPDATE` clause exists to make repeated flushes
    /// additive. If it ever became an overwrite, every flush but the last
    /// would vanish — so this gets its own test.
    #[tokio::test]
    async fn flushing_the_same_key_twice_accumulates() {
        let (store, _bootstrap) = open_test_store().await;
        let key = DailyKey {
            day: "2026-07-30".to_string(),
            tenant_id: "t1".to_string(),
            language: "en_US".to_string(),
            status: 200,
            error_slug: String::new(),
        };
        let latency_key = LatencyKey {
            day: "2026-07-30".to_string(),
            tenant_id: "t1".to_string(),
            bucket_le_ms: 5,
        };

        for _ in 0..3 {
            store
                .flush_usage(
                    vec![(
                        key.clone(),
                        DailyCounters {
                            request_count: 2,
                            latency_sum_us: 4_000,
                        },
                    )],
                    vec![(latency_key.clone(), 2)],
                )
                .await
                .unwrap();
        }

        let rows = store
            .query_usage_daily(None, "2026-07-30", "2026-07-30")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "still one row, not three");
        assert_eq!(rows[0].request_count, 6);
        assert_eq!(rows[0].latency_sum_us, 12_000);

        let latency = store
            .query_usage_latency(None, "2026-07-30", "2026-07-30")
            .await
            .unwrap();
        assert_eq!(latency[0].request_count, 6);
    }

    #[tokio::test]
    async fn usage_queries_filter_by_tenant_and_window() {
        let (store, _bootstrap) = open_test_store().await;
        let row = |tenant: &str, day: &str| {
            (
                DailyKey {
                    day: day.to_string(),
                    tenant_id: tenant.to_string(),
                    language: "en_US".to_string(),
                    status: 200,
                    error_slug: String::new(),
                },
                DailyCounters {
                    request_count: 1,
                    latency_sum_us: 1_000,
                },
            )
        };
        store
            .flush_usage(
                vec![
                    row("t1", "2026-07-30"),
                    row("t2", "2026-07-30"),
                    row("t1", "2026-06-01"),
                ],
                vec![],
            )
            .await
            .unwrap();

        let scoped = store
            .query_usage_daily(Some("t1"), "2026-07-01", "2026-07-31")
            .await
            .unwrap();
        assert_eq!(
            scoped.len(),
            1,
            "other tenant and out-of-window day excluded"
        );
        assert_eq!(scoped[0].request_count, 1);

        let platform = store
            .query_usage_daily(None, "2026-07-01", "2026-07-31")
            .await
            .unwrap();
        assert_eq!(
            platform[0].request_count, 2,
            "platform scope sums across tenants"
        );
    }

    #[tokio::test]
    async fn purge_drops_only_rows_older_than_the_cutoff() {
        let (store, _bootstrap) = open_test_store().await;
        let key = |day: &str| DailyKey {
            day: day.to_string(),
            tenant_id: "t1".to_string(),
            language: "en_US".to_string(),
            status: 200,
            error_slug: String::new(),
        };
        let counters = DailyCounters {
            request_count: 1,
            latency_sum_us: 1_000,
        };
        store
            .flush_usage(
                vec![(key("2026-01-01"), counters), (key("2026-07-30"), counters)],
                vec![(
                    LatencyKey {
                        day: "2026-01-01".to_string(),
                        tenant_id: "t1".to_string(),
                        bucket_le_ms: 5,
                    },
                    1,
                )],
            )
            .await
            .unwrap();

        let purged = store.purge_usage_before("2026-07-01").await.unwrap();
        assert_eq!(purged, 2, "one daily row and one latency row");

        let remaining = store
            .query_usage_daily(None, "2020-01-01", "2030-01-01")
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].day, "2026-07-30");
    }

    #[tokio::test]
    async fn create_tenant_defaults_quota_to_unlimited() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, admin_key) = store
            .create_tenant("Acme".to_string(), None, None, None, None)
            .await
            .unwrap();
        assert_eq!(tenant.quota_limit, 0);
        assert_eq!(admin_key.record.role, Role::Admin);
        assert_eq!(
            admin_key.record.tenant_id.as_deref(),
            Some(tenant.id.as_str())
        );
    }

    #[tokio::test]
    async fn cross_tenant_isolation_on_keys_and_origins() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant_a, _) = store
            .create_tenant("A".to_string(), None, None, None, None)
            .await
            .unwrap();
        let (tenant_b, _) = store
            .create_tenant("B".to_string(), None, None, None, None)
            .await
            .unwrap();

        let key_a = store
            .create_key(&tenant_a.id, "a-key".to_string(), Role::Standard, None)
            .await
            .unwrap();

        assert!(store
            .list_keys(&tenant_a.id)
            .iter()
            .any(|k| k.id == key_a.record.id));
        assert!(!store
            .list_keys(&tenant_b.id)
            .iter()
            .any(|k| k.id == key_a.record.id));

        // Revoking tenant A's key via tenant B's id must fail (not found), not succeed.
        let result = store
            .revoke_key(&tenant_b.id, &key_a.record.id)
            .await
            .unwrap();
        assert!(!result);
        assert!(store.authenticate(&key_a.raw_key).await.is_some());

        store
            .register_origin(&tenant_a.id, "https://a.example.com".to_string())
            .await
            .unwrap();
        assert!(store.tenant_owns_origin(&tenant_a.id, "https://a.example.com"));
        assert!(!store.tenant_owns_origin(&tenant_b.id, "https://a.example.com"));
        assert!(store.is_registered_origin("https://a.example.com"));
    }

    #[tokio::test]
    async fn origin_register_and_revoke() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _) = store
            .create_tenant("Acme".to_string(), None, None, None, None)
            .await
            .unwrap();

        let origin = store
            .register_origin(&tenant.id, "https://app.example.com".to_string())
            .await
            .unwrap();
        assert!(store.is_registered_origin("https://app.example.com"));

        let revoked = store.revoke_origin(&tenant.id, &origin.id).await.unwrap();
        assert!(revoked);
        assert!(!store.is_registered_origin("https://app.example.com"));
    }

    #[tokio::test]
    async fn register_dictionary_and_list() {
        let (store, _bootstrap) = open_test_store().await;

        let info = store
            .register_dictionary("fr_FR".to_string(), "https://example.com/fr".to_string())
            .await
            .unwrap();
        assert_eq!(info.code, "fr_FR");
        assert_eq!(info.source_url, "https://example.com/fr");

        let list = store.list_dictionaries();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].code, "fr_FR");

        // Upsert updates the source_url but preserves the original created_at.
        let original_created_at = info.created_at;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let updated = store
            .register_dictionary("fr_FR".to_string(), "https://example.com/fr2".to_string())
            .await
            .unwrap();
        assert_eq!(updated.source_url, "https://example.com/fr2");
        assert_eq!(updated.created_at, original_created_at);
    }

    #[tokio::test]
    async fn reopen_file_backed_store_preserves_dictionaries() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: dir.path().to_path_buf(),
            refresh_interval_hours: 24,
            dictionary_admin_cidrs: Vec::new(),
            trusted_proxies: Vec::new(),
            db_path: dir.path().join("dict.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };

        {
            let (store, _bootstrap) = Store::open(&config).await.unwrap();
            store
                .register_dictionary("de_DE".to_string(), "https://example.com/de".to_string())
                .await
                .unwrap();
        }

        let (store, _bootstrap) = Store::open(&config).await.unwrap();
        let info = store
            .get_dictionary("de_DE")
            .expect("dictionary survived reopen");
        assert_eq!(info.source_url, "https://example.com/de");
    }

    /// Same code path as the SQLite tests above, run against a real Postgres
    /// instance when one is available. Skips (not fails) if `TEST_DATABASE_URL`
    /// is unset, per DESIGN.md §14 — there's no Postgres in this sandbox, so this
    /// only exercises anything when CI (or a developer) sets the env var.
    #[tokio::test]
    async fn postgres_backend_smoke_test() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping postgres_backend_smoke_test: TEST_DATABASE_URL not set");
            return;
        };

        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect to TEST_DATABASE_URL");
        init_schema(&pool).await.unwrap();

        let store = Store {
            pool,
            keys: RwLock::new(HashMap::new()),
            tenants: RwLock::new(HashMap::new()),
            origins_any: RwLock::new(HashSet::new()),
            origins_by_tenant: RwLock::new(HashMap::new()),
            dictionaries: RwLock::new(HashMap::new()),
        };
        store.reload_caches().await.unwrap();

        let (tenant, admin_key) = store
            .create_tenant("PgTenant".to_string(), None, None, None, None)
            .await
            .unwrap();
        assert_eq!(tenant.quota_limit, 0);
        assert!(store.authenticate(&admin_key.raw_key).await.is_some());

        let revoked = store
            .revoke_key(&tenant.id, &admin_key.record.id)
            .await
            .unwrap();
        assert!(revoked);
        assert!(store.authenticate(&admin_key.raw_key).await.is_none());
    }

    #[tokio::test]
    async fn quota_zero_is_unlimited() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin) = store
            .create_tenant("Acme".to_string(), None, Some(0), None, None)
            .await
            .unwrap();
        for _ in 0..50 {
            assert!(store.try_consume_quota(&tenant.id));
        }
    }

    #[tokio::test]
    async fn quota_blocks_at_limit() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin) = store
            .create_tenant("Acme".to_string(), None, Some(2), None, None)
            .await
            .unwrap();
        assert!(store.try_consume_quota(&tenant.id));
        assert!(store.try_consume_quota(&tenant.id));
        assert!(!store.try_consume_quota(&tenant.id));
    }

    #[tokio::test]
    async fn quota_unblocks_after_limit_raised() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin) = store
            .create_tenant("Acme".to_string(), None, Some(1), None, None)
            .await
            .unwrap();
        assert!(store.try_consume_quota(&tenant.id));
        assert!(!store.try_consume_quota(&tenant.id));

        store
            .update_tenant(&tenant.id, None, None, Some(5), None, None, None)
            .await
            .unwrap();
        assert!(store.try_consume_quota(&tenant.id));
    }

    #[tokio::test]
    async fn quota_unblocks_after_request_count_reset() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin) = store
            .create_tenant("Acme".to_string(), None, Some(1), None, None)
            .await
            .unwrap();
        assert!(store.try_consume_quota(&tenant.id));
        assert!(!store.try_consume_quota(&tenant.id));

        store
            .update_tenant(&tenant.id, None, None, None, Some(0), None, None)
            .await
            .unwrap();
        assert!(store.try_consume_quota(&tenant.id));
    }

    #[tokio::test]
    async fn quota_concurrent_requests_never_overshoot() {
        let (store, _bootstrap) = open_test_store().await;
        let (tenant, _admin) = store
            .create_tenant("Acme".to_string(), None, Some(10), None, None)
            .await
            .unwrap();
        let store = std::sync::Arc::new(store);

        let mut handles = Vec::new();
        for _ in 0..50 {
            let store = store.clone();
            let tenant_id = tenant.id.clone();
            handles.push(tokio::spawn(
                async move { store.try_consume_quota(&tenant_id) },
            ));
        }
        let mut successes = 0;
        for handle in handles {
            if handle.await.unwrap() {
                successes += 1;
            }
        }
        assert_eq!(
            successes, 10,
            "exactly quota_limit requests should succeed, no overshoot"
        );
    }

    #[tokio::test]
    async fn reset_bootstrap_platform_key_creates_on_empty_store() {
        let (store, _bootstrap) = open_test_store().await;
        // Simulate an empty store with no active platform key by revoking the
        // bootstrap key created by `open_test_store`.
        let bootstrap = store
            .keys
            .read()
            .unwrap()
            .values()
            .find(|k| k.role == Role::Platform)
            .cloned()
            .unwrap();
        store.revoke_key("", &bootstrap.id).await.unwrap();
        assert!(store.authenticate("not-the-key").await.is_none());

        let reset = store.reset_bootstrap_platform_key().await.unwrap();
        assert_eq!(reset.record.role, Role::Platform);
        assert!(reset.record.tenant_id.is_none());
        assert_eq!(reset.record.label, "bootstrap");
        assert!(store.authenticate(&reset.raw_key).await.is_some());
    }

    #[tokio::test]
    async fn reset_bootstrap_platform_key_rotates_single_key() {
        let (store, bootstrap) = open_test_store().await;
        let original = bootstrap.expect("bootstrap key should exist");

        let reset = store.reset_bootstrap_platform_key().await.unwrap();
        assert_eq!(reset.record.id, original.record.id);
        assert_eq!(reset.record.label, original.record.label);
        assert_eq!(reset.record.role, Role::Platform);
        assert_ne!(reset.raw_key, original.raw_key);

        assert!(store.authenticate(&original.raw_key).await.is_none());
        assert!(store.authenticate(&reset.raw_key).await.is_some());
    }

    #[tokio::test]
    async fn reset_bootstrap_platform_key_rejects_ambiguous_keys() {
        let (store, _bootstrap) = open_test_store().await;
        // Create a second active bootstrap key to trigger the ambiguity guard.
        store
            .create_key_internal(None, "bootstrap".to_string(), Role::Platform, None)
            .await
            .unwrap();

        let err = store
            .reset_bootstrap_platform_key()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ambiguous"), "{err}");
    }

    /// The CLI reset must survive a real file-backed restart, not just the
    /// in-memory cache — `:memory:` never exercises the reload-from-disk path.
    #[tokio::test]
    async fn reset_bootstrap_platform_key_survives_file_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: dir.path().to_path_buf(),
            refresh_interval_hours: 24,
            dictionary_admin_cidrs: Vec::new(),
            trusted_proxies: Vec::new(),
            db_path: dir.path().join("bootstrap.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };

        let raw_key = {
            let store = Store::open_for_cli(&config).await.unwrap();
            let created = store.reset_bootstrap_platform_key().await.unwrap();
            created.raw_key
        };

        let (store, _bootstrap) = Store::open(&config).await.unwrap();
        assert!(
            store.authenticate(&raw_key).await.is_some(),
            "rotated bootstrap key must be loadable after a file-backed restart"
        );
    }

    /// A running server process must honor a platform-key rotation performed by
    /// a separate CLI process against the same database. The server's in-memory
    /// cache is stale after the rotation, so `authenticate` must fall back to the
    /// database on cache miss instead of returning 401.
    #[tokio::test]
    async fn authenticate_falls_back_to_db_after_external_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: dir.path().to_path_buf(),
            refresh_interval_hours: 24,
            dictionary_admin_cidrs: Vec::new(),
            trusted_proxies: Vec::new(),
            db_path: dir.path().join("stale-cache.db"),
            db_url: None,
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };

        let (server_store, bootstrap) = Store::open(&config).await.unwrap();
        let original = bootstrap.expect("empty store should bootstrap a platform key");

        // A second Store on the same file simulates the offline CLI process.
        let cli_store = Store::open_for_cli(&config).await.unwrap();
        let rotated = cli_store
            .reset_bootstrap_platform_key()
            .await
            .expect("single bootstrap key should rotate cleanly");

        // The new key is not in the running server's in-memory cache, so it is
        // found via the DB fallback path.
        assert!(server_store.authenticate(&rotated.raw_key).await.is_some());
        // Once the new value is cached, the stale hash for the same id is
        // evicted and the old key stops working.
        assert!(server_store.authenticate(&original.raw_key).await.is_none());
    }
}
