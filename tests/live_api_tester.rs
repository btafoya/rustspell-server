//! Live API tester harness.
//!
//! When the `live-tests` feature is enabled, this integration test starts a
//! real server process (or connects to an external one) and runs the bats suite
//! in `tests/bats` against it over actual HTTP calls.

#![cfg(feature = "live-tests")]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const DEFAULT_DICT_URL: &str =
    "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en";
const DEFAULT_BATS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/bats");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_api_tests() {
    require_command("bats");
    require_command("curl");

    let (base_url, platform_key, _server) = match std::env::var("RUSTSPELL_SERVER_URL").ok() {
        Some(url) => {
            let key = std::env::var("RUSTSPELL_PLATFORM_KEY")
                .expect("RUSTSPELL_PLATFORM_KEY is required when RUSTSPELL_SERVER_URL is set");
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

    let bats_dir =
        std::env::var("RUSTSPELL_BATS_DIR").unwrap_or_else(|_| DEFAULT_BATS_DIR.to_string());
    let manifest_path = Path::new(&bats_dir).join("MANIFEST.json");

    let mut cmd = Command::new("bats");
    cmd.arg("--timing")
        .env("RUSTSPELL_SERVER_URL", &base_url)
        .env("RUSTSPELL_PLATFORM_KEY", &platform_key)
        .env("RUSTSPELL_TEST_LOG_FILE", &log_file)
        .env("RUSTSPELL_BATS_MANIFEST", &manifest_path)
        .arg(&bats_dir);

    let status = cmd.status().expect("bats failed to run");
    let passed = status.success();

    if report_mode.contains("json") || report_mode == "all" {
        write_json_report(&report_dir, &base_url, &log_file).expect("write json report");
    }
    if report_mode.contains("junit") || report_mode == "all" {
        write_junit_report(&report_dir, &log_file).expect("write junit report");
    }

    assert!(passed, "bats test suite failed");
}

fn require_command(name: &str) {
    let status = Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap_or_else(|_| panic!("failed to check for {name}"));
    assert!(
        status.success(),
        "live-tests feature requires `{name}` to be installed and on PATH"
    );
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
    let metrics_url = format!("http://127.0.0.1:{}", api_port.1);

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
        metrics_url,
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
    let deadline = Instant::now() + Duration::from_secs(120);
    let health_url = format!("{}/health", url);
    while Instant::now() < deadline {
        if let Ok(output) = Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--max-time",
                "5",
                &health_url,
            ])
            .output()
        {
            if output.status.success() {
                let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if code == "200" {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("server did not become healthy within 120s at {url}");
}

struct ServerGuard {
    child: Child,
    url: String,
    metrics_url: String,
    platform_key: String,
    bin: PathBuf,
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
        "timestamp": now_iso(),
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
    let mut by_operation: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
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

fn now_iso() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", secs)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
