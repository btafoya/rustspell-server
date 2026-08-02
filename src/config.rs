//! Application configuration loaded from environment variables.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

/// Default public API port.
pub const DEFAULT_PORT: u16 = 3000;
/// Default Prometheus metrics port.
pub const DEFAULT_METRICS_PORT: u16 = 9090;
/// Default log level filter.
pub const DEFAULT_LOG_LEVEL: &str = "info";
/// Default dictionary language.
pub const DEFAULT_LANGUAGE: &str = "en_US";
/// Default dictionary refresh interval in hours.
pub const DEFAULT_REFRESH_INTERVAL_HOURS: u64 = 24;
/// Default base URL for raw `.aff`/`.dic` dictionary files.
pub const DEFAULT_DICTIONARY_URL: &str =
    "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en";
/// Default auth failures allowed per IP per window before a cooldown.
pub const DEFAULT_AUTH_RATE_LIMIT_MAX: u32 = 10;
/// Default sliding window (seconds) for counting auth failures.
pub const DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECONDS: u64 = 60;
/// Default cooldown (seconds) once the failure threshold is exceeded.
pub const DEFAULT_AUTH_RATE_LIMIT_COOLDOWN_SECONDS: u64 = 60;

/// An IP network in CIDR notation (`10.0.0.0/8`, `2001:db8::/32`), or a bare
/// address, which is treated as a host route (`/32` or `/128`).
///
/// Hand-rolled rather than pulled from a crate because the part that actually
/// bites in production — normalizing the IPv4-mapped IPv6 addresses a
/// dual-stack listener hands you (`::ffff:10.0.0.5`) — has to be written
/// either way. See [`canonical_ip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parse `addr/prefix`, or a bare `addr` as a single-host network.
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let (addr_part, prefix_part) = match spec.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (spec, None),
        };

        let addr = canonical_ip(
            addr_part
                .parse::<IpAddr>()
                .map_err(|_| anyhow::anyhow!("invalid IP address in CIDR {spec:?}"))?,
        );
        let max_prefix = if addr.is_ipv4() { 32 } else { 128 };

        let prefix = match prefix_part {
            Some(text) => text
                .parse::<u8>()
                .map_err(|_| anyhow::anyhow!("invalid prefix length in CIDR {spec:?}"))?,
            None => max_prefix,
        };
        if prefix > max_prefix {
            anyhow::bail!("prefix /{prefix} exceeds /{max_prefix} in CIDR {spec:?}");
        }

        // Store the masked network so a spec with host bits set (10.0.0.5/8)
        // still matches every address in its range.
        Ok(Self {
            network: mask_ip(addr, prefix),
            prefix,
        })
    }

    /// Whether `ip` falls inside this network. A v4 address never matches a
    /// v6 network or vice versa, once both are canonicalized.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        if ip.is_ipv4() != self.network.is_ipv4() {
            return false;
        }
        mask_ip(ip, self.prefix) == self.network
    }
}

/// Collapse an IPv4-mapped IPv6 address to its IPv4 form, so a peer arriving
/// as `::ffff:10.0.0.5` on a dual-stack listener matches a `10.0.0.0/8` rule.
/// Uses `to_ipv4_mapped` rather than `to_ipv4`, which would also rewrite
/// IPv4-*compatible* addresses like `::1` into a misleading `0.0.0.1`.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Zero every host bit below `prefix`. The `prefix == 0` arms exist because
/// shifting an integer by its full width is undefined and panics in debug.
fn mask_ip(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let masked = if prefix == 0 {
                0
            } else {
                u32::from(v4) & (!0u32 << (32 - prefix))
            };
            IpAddr::V4(Ipv4Addr::from(masked))
        }
        IpAddr::V6(v6) => {
            let masked = if prefix == 0 {
                0
            } else {
                u128::from(v6) & (!0u128 << (128 - prefix))
            };
            IpAddr::V6(Ipv6Addr::from(masked))
        }
    }
}

/// Runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Public API port.
    pub port: u16,
    /// Prometheus metrics port.
    pub metrics_port: u16,
    /// `tracing` env-filter directive.
    pub log_level: String,
    /// Dictionary locale (e.g. `en_US`).
    pub language: String,
    /// Base URL from which to download `{language}.aff` and `{language}.dic`.
    pub dictionary_url: String,
    /// Directory where extracted `.aff`/`.dic` files are cached.
    pub dictionary_dir: PathBuf,
    /// Re-download if local files are older than this many hours.
    pub refresh_interval_hours: u64,
    /// SQLite file for the key/tenant store, used when `db_url` is unset.
    pub db_path: PathBuf,
    /// PostgreSQL connection string. When set, takes precedence over `db_path`.
    pub db_url: Option<String>,
    /// Auth failures allowed per IP per window before a cooldown.
    pub auth_rate_limit_max: u32,
    /// Sliding window (seconds) for counting auth failures.
    pub auth_rate_limit_window_seconds: u64,
    /// Cooldown (seconds) once the failure threshold is exceeded.
    pub auth_rate_limit_cooldown_seconds: u64,
    /// Networks permitted to call `POST /dictionaries`. Empty means no network
    /// restriction — the platform key alone gates the endpoint (§27.3).
    pub dictionary_admin_cidrs: Vec<Cidr>,
    /// Reverse proxies whose `X-Forwarded-For` header may be trusted when
    /// resolving the caller of `POST /dictionaries`. Empty means the header is
    /// never consulted and the TCP peer is always used.
    pub trusted_proxies: Vec<Cidr>,
}

/// Load and validate configuration from the environment.
pub fn load() -> anyhow::Result<Config> {
    let port = parse_env_or("RUSTSPELL_PORT", DEFAULT_PORT)?;
    let metrics_port = parse_env_or("RUSTSPELL_METRICS_PORT", DEFAULT_METRICS_PORT)?;

    if port == metrics_port {
        anyhow::bail!(
            "RUSTSPELL_PORT ({port}) and RUSTSPELL_METRICS_PORT ({metrics_port}) must be different"
        );
    }

    let dictionary_dir = std::env::var_os("RUSTSPELL_DICTIONARY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_dictionary_dir);

    let db_path = std::env::var_os("RUSTSPELL_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path);

    let db_url = match std::env::var("RUSTSPELL_DB_URL") {
        Ok(url) if !url.is_empty() => {
            if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                anyhow::bail!("RUSTSPELL_DB_URL must be a postgres:// connection string");
            }
            Some(url)
        }
        _ => None,
    };

    Ok(Config {
        port,
        metrics_port,
        log_level: std::env::var("RUSTSPELL_LOG_LEVEL")
            .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string()),
        language: std::env::var("RUSTSPELL_LANGUAGE")
            .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string()),
        dictionary_url: std::env::var("RUSTSPELL_DICTIONARY_URL")
            .unwrap_or_else(|_| DEFAULT_DICTIONARY_URL.to_string()),
        dictionary_dir,
        refresh_interval_hours: parse_env_or(
            "RUSTSPELL_REFRESH_INTERVAL_HOURS",
            DEFAULT_REFRESH_INTERVAL_HOURS,
        )?,
        db_path,
        db_url,
        auth_rate_limit_max: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_MAX",
            DEFAULT_AUTH_RATE_LIMIT_MAX,
        )?,
        auth_rate_limit_window_seconds: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS",
            DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECONDS,
        )?,
        auth_rate_limit_cooldown_seconds: parse_env_or(
            "RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS",
            DEFAULT_AUTH_RATE_LIMIT_COOLDOWN_SECONDS,
        )?,
        dictionary_admin_cidrs: parse_cidr_list("RUSTSPELL_DICTIONARY_ADMIN_CIDRS")?,
        trusted_proxies: parse_cidr_list("RUSTSPELL_TRUSTED_PROXIES")?,
    })
}

/// Parse a comma-separated CIDR list, failing startup on a malformed entry
/// rather than silently dropping it — a typo in an allow-list must not quietly
/// widen or narrow access.
fn parse_cidr_list(name: &str) -> anyhow::Result<Vec<Cidr>> {
    match std::env::var(name) {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| Cidr::parse(entry).map_err(|e| anyhow::anyhow!("invalid {name}: {e}")))
            .collect(),
        Err(std::env::VarError::NotPresent) => Ok(Vec::new()),
        Err(e) => Err(anyhow::anyhow!("failed to read {name}: {e}")),
    }
}

fn parse_env_or<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid {name}: {e}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(anyhow::anyhow!("failed to read {name}: {e}")),
    }
}

fn default_dictionary_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "rustspell", "rustspell-server")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn default_db_path() -> PathBuf {
    directories::ProjectDirs::from("com", "rustspell", "rustspell-server")
        .map(|dirs| dirs.data_dir().join("rustspell.db"))
        .unwrap_or_else(|| PathBuf::from("./data/rustspell.db"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    // Environment tests mutate process-global state; serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_are_valid() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();

        let config = load().expect("load should succeed");
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.metrics_port, DEFAULT_METRICS_PORT);
        assert_eq!(config.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.language, DEFAULT_LANGUAGE);
        assert_eq!(
            config.refresh_interval_hours,
            DEFAULT_REFRESH_INTERVAL_HOURS
        );
        assert_eq!(config.dictionary_url, DEFAULT_DICTIONARY_URL);
    }

    #[test]
    fn rejects_equal_ports() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_PORT", "3000");
        std::env::set_var("RUSTSPELL_METRICS_PORT", "3000");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("must be different"));
    }

    #[test]
    fn rejects_non_postgres_db_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_DB_URL", "mysql://localhost/db");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("RUSTSPELL_DB_URL"));
    }

    fn clear_env() {
        for key in [
            "RUSTSPELL_PORT",
            "RUSTSPELL_METRICS_PORT",
            "RUSTSPELL_LOG_LEVEL",
            "RUSTSPELL_LANGUAGE",
            "RUSTSPELL_DICTIONARY_URL",
            "RUSTSPELL_DICTIONARY_DIR",
            "RUSTSPELL_REFRESH_INTERVAL_HOURS",
            "RUSTSPELL_DB_PATH",
            "RUSTSPELL_DB_URL",
            "RUSTSPELL_DICTIONARY_ADMIN_CIDRS",
            "RUSTSPELL_TRUSTED_PROXIES",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn cidr_lists_default_to_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();

        let config = load().expect("load should succeed");
        assert!(config.dictionary_admin_cidrs.is_empty());
        assert!(config.trusted_proxies.is_empty());
    }

    #[test]
    fn rejects_malformed_cidr_list() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_DICTIONARY_ADMIN_CIDRS", "10.0.0.0/8,not-an-ip");

        let err = load().unwrap_err().to_string();
        assert!(err.contains("RUSTSPELL_DICTIONARY_ADMIN_CIDRS"), "{err}");
    }

    #[test]
    fn parses_comma_separated_cidrs_with_whitespace() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("RUSTSPELL_TRUSTED_PROXIES", " 172.17.0.0/16 , ::1 ");

        let config = load().expect("load should succeed");
        assert_eq!(config.trusted_proxies.len(), 2);
        assert!(config.trusted_proxies[0].contains("172.17.0.1".parse().unwrap()));
        assert!(config.trusted_proxies[1].contains("::1".parse().unwrap()));
    }

    #[test]
    fn cidr_matches_within_range_and_rejects_outside() {
        let net = Cidr::parse("10.20.0.0/24").unwrap();
        assert!(net.contains("10.20.0.1".parse().unwrap()));
        assert!(net.contains("10.20.0.255".parse().unwrap()));
        assert!(!net.contains("10.20.1.1".parse().unwrap()));
        assert!(!net.contains("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn cidr_ignores_host_bits_in_the_spec() {
        let sloppy = Cidr::parse("10.20.0.7/24").unwrap();
        let clean = Cidr::parse("10.20.0.0/24").unwrap();
        assert_eq!(sloppy, clean);
        assert!(sloppy.contains("10.20.0.1".parse().unwrap()));
    }

    #[test]
    fn bare_address_is_a_host_route() {
        let host = Cidr::parse("203.0.113.9").unwrap();
        assert!(host.contains("203.0.113.9".parse().unwrap()));
        assert!(!host.contains("203.0.113.10".parse().unwrap()));
    }

    #[test]
    fn zero_prefix_matches_everything_of_its_family() {
        let all_v4 = Cidr::parse("0.0.0.0/0").unwrap();
        assert!(all_v4.contains("1.2.3.4".parse().unwrap()));
        assert!(all_v4.contains("203.0.113.9".parse().unwrap()));
        // A v6 address stays outside a v4 network.
        assert!(!all_v4.contains("2001:db8::1".parse().unwrap()));

        let all_v6 = Cidr::parse("::/0").unwrap();
        assert!(all_v6.contains("2001:db8::1".parse().unwrap()));
        assert!(!all_v6.contains("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn full_width_prefixes_do_not_overflow() {
        let v4 = Cidr::parse("192.0.2.1/32").unwrap();
        assert!(v4.contains("192.0.2.1".parse().unwrap()));
        assert!(!v4.contains("192.0.2.2".parse().unwrap()));

        let v6 = Cidr::parse("2001:db8::1/128").unwrap();
        assert!(v6.contains("2001:db8::1".parse().unwrap()));
        assert!(!v6.contains("2001:db8::2".parse().unwrap()));
    }

    #[test]
    fn v4_mapped_v6_peer_matches_a_v4_network() {
        // What a dual-stack listener actually reports for an IPv4 client.
        let net = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(net.contains("::ffff:10.0.0.5".parse().unwrap()));
    }

    #[test]
    fn v4_compatible_v6_is_not_mistaken_for_v4() {
        // `to_ipv4` would turn ::1 into 0.0.0.1 and match a 0.0.0.0/8 rule;
        // `to_ipv4_mapped` must not.
        let net = Cidr::parse("0.0.0.0/8").unwrap();
        assert!(!net.contains("::1".parse().unwrap()));
    }

    #[test]
    fn rejects_oversized_prefix() {
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        assert!(Cidr::parse("2001:db8::/129").is_err());
        assert!(Cidr::parse("10.0.0.0/abc").is_err());
        assert!(Cidr::parse("not-an-ip/8").is_err());
    }
}
