//! Pure-Rust live API tester.
//!
//! When the `live-tests` feature is enabled, this integration test starts a
//! real server process (or connects to an external one) and exercises every
//! public operation over actual HTTP calls. No shell scripting or external
//! test runners are required.

#![cfg(feature = "live-tests")]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::{Client, Method};
use serde_json::{json, Value};

const DEFAULT_DICT_URL: &str =
    "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_api_tests() {
    let (base_url, platform_key, _server) = match std::env::var("RUSTSPELL_SERVER_URL").ok() {
        Some(url) => {
            let key = match std::env::var("RUSTSPELL_PLATFORM_KEY").ok() {
                Some(key) => key,
                None => {
                    let path = std::env::var("RUSTSPELL_PLATFORM_KEY_FILE").expect(
                        "RUSTSPELL_PLATFORM_KEY or RUSTSPELL_PLATFORM_KEY_FILE is required when RUSTSPELL_SERVER_URL is set",
                    );
                    read_platform_key_from_file(&path)
                }
            };
            (url, key, None)
        }
        None => {
            let guard = spawn_server().await;
            (guard.url.clone(), guard.platform_key.clone(), Some(guard))
        }
    };

    let report_mode =
        std::env::var("RUSTSPELL_TEST_REPORT").unwrap_or_else(|_| "console".to_string());
    let report_dir = std::env::var("RUSTSPELL_TEST_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/live-test-reports"));
    fs::create_dir_all(&report_dir).expect("create report dir");

    let log_file = report_dir.join("live-test-log.jsonl");
    let _ = fs::remove_file(&log_file);

    let client = ApiClient::new(&base_url, log_file.clone());

    // --- health ---------------------------------------------------------------
    let body = client
        .call("healthCheck", Method::GET, "/health", 200, None, None)
        .await;
    assert_eq!(body["status"].as_str(), Some("ok"));

    let body = client
        .call(
            "healthCheck",
            Method::GET,
            "/health?verbose=true",
            200,
            None,
            None,
        )
        .await;
    assert_eq!(body["status"].as_str(), Some("ok"));
    assert!(!body["uptime_seconds"].is_null());
    assert!(!body["request_count"].is_null());

    // --- docs -----------------------------------------------------------------
    let body = client
        .call("getOpenApiSpec", Method::GET, "/docs", 200, None, None)
        .await;
    assert_eq!(body["openapi"].as_str(), Some("3.0.3"));
    assert_eq!(body["info"]["title"].as_str(), Some("Rust Spell Server"));

    // --- spellcheck -----------------------------------------------------------
    let standard_key = client.create_tenant_key("standard", &platform_key).await;

    let body = client
        .call(
            "spellcheck",
            Method::POST,
            "/spellcheck",
            200,
            Some(json!({"text":"hello world"})),
            Some(&standard_key),
        )
        .await;
    assert_eq!(body["results"].as_array().map(|a| a.len()), Some(2));
    assert_eq!(body["results"][0]["token"].as_str(), Some("hello"));
    assert_eq!(body["results"][0]["valid"].as_bool(), Some(true));
    assert_eq!(
        body["results"][0]["suggestions"]
            .as_array()
            .map(|a| a.len()),
        Some(0)
    );

    let body = client
        .call(
            "spellcheck",
            Method::POST,
            "/spellcheck",
            200,
            Some(json!({"words":["hello","helo"]})),
            Some(&standard_key),
        )
        .await;
    assert_eq!(body["results"].as_array().map(|a| a.len()), Some(2));
    assert_eq!(body["results"][0]["valid"].as_bool(), Some(true));
    assert_eq!(body["results"][1]["valid"].as_bool(), Some(false));

    let body = client
        .call(
            "spellcheck",
            Method::POST,
            "/spellcheck",
            400,
            Some(json!({})),
            Some(&standard_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(400));

    let body = client
        .call(
            "spellcheck",
            Method::POST,
            "/spellcheck",
            401,
            Some(json!({"text":"hello"})),
            None,
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(401));

    let body = client
        .call(
            "spellcheckPositions",
            Method::POST,
            "/spellcheck/positions",
            200,
            Some(json!({"text":"helo world helo"})),
            Some(&standard_key),
        )
        .await;
    assert_eq!(body["results"].as_array().map(|a| a.len()), Some(1));
    assert_eq!(body["results"][0]["token"].as_str(), Some("helo"));
    assert_eq!(
        body["results"][0]["positions"].as_array().map(|a| a.len()),
        Some(2)
    );

    let body = client
        .call(
            "spellcheckPositions",
            Method::POST,
            "/spellcheck/positions",
            400,
            Some(json!({})),
            Some(&standard_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(400));

    let body = client
        .call(
            "spellcheckPositions",
            Method::POST,
            "/spellcheck/positions",
            401,
            Some(json!({"text":"hello"})),
            None,
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(401));

    // --- api keys -------------------------------------------------------------
    let admin_key = client.create_admin_key(&platform_key).await;
    let standard_key_for_keys = client.create_tenant_key("standard", &platform_key).await;

    let body = client
        .call(
            "createApiKey",
            Method::POST,
            "/api-keys",
            200,
            Some(json!({"label":"new-key","role":"admin"})),
            Some(&admin_key),
        )
        .await;
    assert!(!body["key"].as_str().unwrap_or("").is_empty());
    assert_eq!(body["label"].as_str(), Some("new-key"));
    assert_eq!(body["role"].as_str(), Some("admin"));

    let body = client
        .call(
            "createApiKey",
            Method::POST,
            "/api-keys",
            400,
            Some(json!({"label":""})),
            Some(&admin_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(400));

    let body = client
        .call(
            "createApiKey",
            Method::POST,
            "/api-keys",
            403,
            Some(json!({"label":"nope","role":"admin"})),
            Some(&standard_key_for_keys),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    let body = client
        .call(
            "listApiKeys",
            Method::GET,
            "/api-keys",
            200,
            None,
            Some(&admin_key),
        )
        .await;
    assert_eq!(body["keys"].as_array().map(|_| ()), Some(()));

    let body = client
        .call(
            "listApiKeys",
            Method::GET,
            "/api-keys",
            403,
            None,
            Some(&standard_key_for_keys),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    let key_result = client
        .call(
            "createApiKey",
            Method::POST,
            "/api-keys",
            200,
            Some(json!({"label":"to-revoke","role":"standard"})),
            Some(&admin_key),
        )
        .await;
    let key_id = key_result["id"]
        .as_str()
        .expect("revoke key id")
        .to_string();

    client
        .call(
            "revokeApiKey",
            Method::DELETE,
            &format!("/api-keys/{key_id}"),
            204,
            None,
            Some(&admin_key),
        )
        .await;
    client
        .call(
            "revokeApiKey",
            Method::DELETE,
            &format!("/api-keys/{key_id}"),
            204,
            None,
            Some(&admin_key),
        )
        .await;

    let body = client
        .call(
            "revokeApiKey",
            Method::DELETE,
            "/api-keys/00000000-0000-0000-0000-000000000000",
            404,
            None,
            Some(&admin_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    let key_result = client
        .call(
            "createApiKey",
            Method::POST,
            "/api-keys",
            200,
            Some(json!({"label":"to-rotate","role":"standard"})),
            Some(&admin_key),
        )
        .await;
    let key_id = key_result["id"]
        .as_str()
        .expect("rotate key id")
        .to_string();
    let old_key = key_result["key"]
        .as_str()
        .expect("rotate old key")
        .to_string();

    let body = client
        .call(
            "rotateApiKey",
            Method::POST,
            &format!("/api-keys/{key_id}/rotate"),
            200,
            None,
            Some(&admin_key),
        )
        .await;
    let new_key = body["key"].as_str().expect("rotate new key").to_string();
    assert!(!new_key.is_empty());
    assert_ne!(new_key, old_key);
    assert_eq!(body["id"].as_str(), Some(key_id.as_str()));

    let body = client
        .call(
            "rotateApiKey",
            Method::POST,
            "/api-keys/00000000-0000-0000-0000-000000000000/rotate",
            404,
            None,
            Some(&admin_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    // --- tenant lifecycle -----------------------------------------------------
    let (tenant_id, tenant_admin_key) = client.create_tenant(&platform_key).await;

    let body = client
        .call(
            "getOwnTenant",
            Method::GET,
            "/tenant",
            200,
            None,
            Some(&tenant_admin_key),
        )
        .await;
    assert_eq!(body["id"].as_str(), Some(tenant_id.as_str()));

    let body = client
        .call("getOwnTenant", Method::GET, "/tenant", 401, None, None)
        .await;
    assert_eq!(body["status"].as_i64(), Some(401));

    let body = client
        .call(
            "createTenant",
            Method::POST,
            "/tenants",
            200,
            Some(json!({"name":"Create Test"})),
            Some(&platform_key),
        )
        .await;
    assert!(!body["id"].as_str().unwrap_or("").is_empty());
    assert!(!body["admin_key"]["key"].as_str().unwrap_or("").is_empty());

    let body = client
        .call(
            "createTenant",
            Method::POST,
            "/tenants",
            400,
            Some(json!({"name":""})),
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(400));

    let body = client
        .call_with_headers(
            "createTenant",
            Method::POST,
            "/tenants",
            403,
            Some(json!({"name":"Origin Test"})),
            Some(&platform_key),
            &[("Origin", "https://example.com")],
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    let body = client
        .call(
            "listTenants",
            Method::GET,
            "/tenants",
            200,
            None,
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["tenants"].as_array().map(|_| ()), Some(()));

    let body = client
        .call(
            "getTenant",
            Method::GET,
            &format!("/tenants/{tenant_id}"),
            200,
            None,
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["id"].as_str(), Some(tenant_id.as_str()));

    let body = client
        .call(
            "getTenant",
            Method::GET,
            "/tenants/00000000-0000-0000-0000-000000000000",
            404,
            None,
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    let body = client
        .call(
            "updateTenant",
            Method::PATCH,
            &format!("/tenants/{tenant_id}"),
            200,
            Some(json!({"name":"Updated Tenant"})),
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["name"].as_str(), Some("Updated Tenant"));

    let body = client
        .call(
            "updateTenant",
            Method::PATCH,
            "/tenants/00000000-0000-0000-0000-000000000000",
            404,
            Some(json!({"name":"Nope"})),
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    client
        .call(
            "suspendTenant",
            Method::POST,
            &format!("/tenants/{tenant_id}/suspend"),
            204,
            None,
            Some(&platform_key),
        )
        .await;

    let body = client
        .call(
            "getOwnTenant",
            Method::GET,
            "/tenant",
            403,
            None,
            Some(&tenant_admin_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    client
        .call(
            "reactivateTenant",
            Method::POST,
            &format!("/tenants/{tenant_id}/reactivate"),
            204,
            None,
            Some(&platform_key),
        )
        .await;

    let body = client
        .call(
            "getOwnTenant",
            Method::GET,
            "/tenant",
            200,
            None,
            Some(&tenant_admin_key),
        )
        .await;
    assert_eq!(body["id"].as_str(), Some(tenant_id.as_str()));

    let body = client
        .call(
            "suspendTenant",
            Method::POST,
            "/tenants/00000000-0000-0000-0000-000000000000/suspend",
            404,
            None,
            Some(&platform_key),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    // --- origins --------------------------------------------------------------
    let admin_key_for_origins = client.create_admin_key(&platform_key).await;
    let standard_key_for_origins = client.create_tenant_key("standard", &platform_key).await;

    let body = client
        .call(
            "listOwnOrigins",
            Method::GET,
            "/tenant/origins",
            403,
            None,
            Some(&standard_key_for_origins),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    let body = client
        .call(
            "listOwnOrigins",
            Method::GET,
            "/tenant/origins",
            200,
            None,
            Some(&admin_key_for_origins),
        )
        .await;
    assert_eq!(body["origins"].as_array().map(|_| ()), Some(()));

    let body = client
        .call(
            "registerOrigin",
            Method::POST,
            "/tenant/origins",
            200,
            Some(json!({"origin":"https://app.example.com"})),
            Some(&admin_key_for_origins),
        )
        .await;
    assert_eq!(body["origin"].as_str(), Some("https://app.example.com"));

    let body = client
        .call(
            "registerOrigin",
            Method::POST,
            "/tenant/origins",
            400,
            Some(json!({"origin":""})),
            Some(&admin_key_for_origins),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(400));

    let body = client
        .call(
            "registerOrigin",
            Method::POST,
            "/tenant/origins",
            403,
            Some(json!({"origin":"https://x.example.com"})),
            Some(&standard_key_for_origins),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(403));

    let origin_result = client
        .call(
            "registerOrigin",
            Method::POST,
            "/tenant/origins",
            200,
            Some(json!({"origin":"https://revoke.example.com"})),
            Some(&admin_key_for_origins),
        )
        .await;
    let origin_id = origin_result["id"].as_str().expect("origin id").to_string();

    client
        .call(
            "revokeOrigin",
            Method::DELETE,
            &format!("/tenant/origins/{origin_id}"),
            204,
            None,
            Some(&admin_key_for_origins),
        )
        .await;

    let body = client
        .call(
            "revokeOrigin",
            Method::DELETE,
            &format!("/tenant/origins/{origin_id}"),
            404,
            None,
            Some(&admin_key_for_origins),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    let body = client
        .call(
            "revokeOrigin",
            Method::DELETE,
            "/tenant/origins/00000000-0000-0000-0000-000000000000",
            404,
            None,
            Some(&admin_key_for_origins),
        )
        .await;
    assert_eq!(body["status"].as_i64(), Some(404));

    // --- reports --------------------------------------------------------------
    if report_mode.contains("json") || report_mode == "all" {
        write_json_report(&report_dir, &base_url, &log_file).expect("write json report");
    }
    if report_mode.contains("junit") || report_mode == "all" {
        write_junit_report(&report_dir, &log_file).expect("write junit report");
    }
}

#[derive(Debug, serde::Serialize)]
struct LogEntry {
    operation_id: String,
    method: String,
    path: String,
    expected_status: u16,
    actual_status: u16,
    passed: bool,
}

struct ApiClient {
    client: Client,
    base_url: String,
    log_file: Option<PathBuf>,
}

impl ApiClient {
    fn new(base_url: &str, log_file: PathBuf) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            log_file: Some(log_file),
        }
    }

    async fn call(
        &self,
        operation_id: &str,
        method: Method,
        path: &str,
        expected_status: u16,
        body: Option<Value>,
        api_key: Option<&str>,
    ) -> Value {
        self.call_with_headers(
            operation_id,
            method,
            path,
            expected_status,
            body,
            api_key,
            &[],
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn call_with_headers(
        &self,
        operation_id: &str,
        method: Method,
        path: &str,
        expected_status: u16,
        body: Option<Value>,
        api_key: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> Value {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method.clone(), &url);
        if let Some(key) = api_key {
            req = req.header("X-API-Key", key);
        }
        for (name, value) in extra_headers {
            req = req.header(*name, *value);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }

        let resp = req
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{operation_id} {method} {path} request failed: {e}"));
        let actual_status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let passed = actual_status == expected_status;

        self.log(LogEntry {
            operation_id: operation_id.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            expected_status,
            actual_status,
            passed,
        });

        assert!(
            passed,
            "{operation_id} {method} {path} expected {expected_status} but got {actual_status}\nbody: {text}",
        );

        if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|e| {
                panic!("{operation_id} {method} {path} returned invalid JSON: {e}\nbody: {text}")
            })
        }
    }

    fn log(&self, entry: LogEntry) {
        if let Some(log_file) = &self.log_file {
            if let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)
            {
                let line = serde_json::to_string(&entry).expect("serialize log entry");
                let _ = writeln!(file, "{line}");
            }
        }
    }

    async fn create_tenant(&self, platform_key: &str) -> (String, String) {
        let body = self
            .call(
                "createTenant",
                Method::POST,
                "/tenants",
                200,
                Some(json!({"name":"Live Test Tenant"})),
                Some(platform_key),
            )
            .await;
        let id = body["id"]
            .as_str()
            .expect("createTenant response missing id")
            .to_string();
        let admin_key = body["admin_key"]["key"]
            .as_str()
            .expect("createTenant response missing admin_key.key")
            .to_string();
        (id, admin_key)
    }

    async fn create_admin_key(&self, platform_key: &str) -> String {
        self.create_tenant(platform_key).await.1
    }

    async fn create_tenant_key(&self, role: &str, platform_key: &str) -> String {
        let admin_key = self.create_admin_key(platform_key).await;
        if role == "admin" {
            return admin_key;
        }
        let body = self
            .call(
                "createApiKey",
                Method::POST,
                "/api-keys",
                200,
                Some(json!({"label":"standard-key","role":"standard"})),
                Some(&admin_key),
            )
            .await;
        body["key"]
            .as_str()
            .expect("createApiKey response missing key")
            .to_string()
    }
}

fn read_platform_key_from_file(path: &str) -> String {
    let secrets =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("read platform key file {path:?}: {e}"));
    serde_json::from_str::<Value>(&secrets)
        .expect("invalid platform key JSON")
        .get("platform_key")
        .and_then(|v| v.as_str())
        .expect("platform key file missing platform_key")
        .to_string()
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind temporary listener");
    listener.local_addr().expect("local addr").port()
}

async fn spawn_server() -> ServerGuard {
    let api_port = loop {
        let port = free_port();
        let metrics_port = free_port();
        if port != metrics_port {
            break (port, metrics_port);
        }
    };

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("rustspell.db");
    let dict_dir = temp_dir.path().join("dictionaries");
    let secrets_path = temp_dir.path().join("bootstrap.json");

    let bin = server_binary_path();
    let mut cmd = Command::new(&bin);
    cmd.env("RUSTSPELL_PORT", api_port.0.to_string())
        .env("RUSTSPELL_METRICS_PORT", api_port.1.to_string())
        .env("RUSTSPELL_DB_PATH", &db_path)
        .env("RUSTSPELL_DICTIONARY_DIR", &dict_dir)
        .env("RUSTSPELL_DICTIONARY_URL", DEFAULT_DICT_URL)
        .env("RUSTSPELL_BOOTSTRAP_SECRETS_PATH", &secrets_path)
        .env("RUSTSPELL_LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("spawn server {bin:?}: {e}"));

    let url = format!("http://127.0.0.1:{}", api_port.0);

    wait_for_health(&url).await;

    let secrets = fs::read_to_string(&secrets_path)
        .unwrap_or_else(|_| panic!("bootstrap secrets file missing at {secrets_path:?}"));
    let platform_key = serde_json::from_str::<Value>(&secrets)
        .expect("invalid bootstrap secrets JSON")
        .get("platform_key")
        .and_then(|v| v.as_str())
        .expect("bootstrap secrets missing platform_key")
        .to_string();

    ServerGuard {
        child,
        url,
        platform_key,
        _temp_dir: temp_dir,
    }
}

fn server_binary_path() -> PathBuf {
    if let Ok(bin) = std::env::var("RUSTSPELL_SERVER_BIN") {
        return PathBuf::from(bin);
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let candidate = PathBuf::from(format!("target/{}/rustspell-server", profile));
    if candidate.exists() {
        return candidate;
    }
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--quiet", "--bin", "rustspell-server"]);
    if profile == "release" {
        cmd.arg("--release");
    }
    let status = cmd.status().expect("cargo build failed");
    assert!(status.success(), "cargo build failed");
    assert!(candidate.exists(), "server binary missing after build");
    candidate
}

async fn wait_for_health(url: &str) {
    let client = Client::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let health_url = format!("{}/health", url);
    while Instant::now() < deadline {
        if let Ok(resp) = client
            .get(&health_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("server did not become healthy within 120s at {url}");
}

struct ServerGuard {
    child: Child,
    url: String,
    platform_key: String,
    _temp_dir: tempfile::TempDir,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_json_report(report_dir: &Path, base_url: &str, log_file: &Path) -> std::io::Result<()> {
    let mut entries = Vec::new();
    if log_file.exists() {
        for line in fs::read_to_string(log_file)?.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                entries.push(v);
            }
        }
    }
    let summary = summarize_entries(&entries);
    let report = serde_json::json!({
        "timestamp": now_timestamp(),
        "server_url": base_url,
        "entries": entries,
        "summary": summary,
    });
    fs::write(
        report_dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )
}

fn write_junit_report(report_dir: &Path, log_file: &Path) -> std::io::Result<()> {
    let mut by_operation: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    if log_file.exists() {
        for line in fs::read_to_string(log_file)?.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                let op = v
                    .get("operation_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                by_operation.entry(op).or_default().push(v);
            }
        }
    }

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let total_tests: usize = by_operation.values().map(|v| v.len()).sum();
    let failures: usize = by_operation
        .values()
        .flatten()
        .filter(|v| v.get("passed").and_then(|x| x.as_bool()) == Some(false))
        .count();
    xml.push_str(&format!(
        "<testsuites name=\"rustspell-live-api\" tests=\"{}\" failures=\"{}\">\n",
        total_tests, failures
    ));
    for (op, cases) in by_operation {
        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\">\n",
            escape_xml(&op),
            cases.len()
        ));
        for case in cases {
            let method = case["method"].as_str().unwrap_or("?");
            let path = case["path"].as_str().unwrap_or("?");
            let expected = case["expected_status"].as_i64().unwrap_or(-1);
            let actual = case["actual_status"].as_i64().unwrap_or(-1);
            let passed = case["passed"].as_bool().unwrap_or(false);
            let name = format!("{} {} {} -> {}", method, path, op, expected);
            xml.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\">\n",
                escape_xml(&name),
                escape_xml(&op)
            ));
            if !passed {
                xml.push_str(&format!(
                    "      <failure>expected {} but got {}</failure>\n",
                    expected, actual
                ));
            }
            xml.push_str("    </testcase>\n");
        }
        xml.push_str("  </testsuite>\n");
    }
    xml.push_str("</testsuites>\n");
    fs::write(report_dir.join("junit.xml"), xml)
}

fn summarize_entries(entries: &[Value]) -> Value {
    let total = entries.len();
    let passed = entries
        .iter()
        .filter(|v| v.get("passed").and_then(|x| x.as_bool()) == Some(true))
        .count();
    serde_json::json!({
        "total": total,
        "passed": passed,
        "failed": total - passed,
    })
}

fn now_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
