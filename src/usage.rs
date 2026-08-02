//! Usage rollup: in-memory accumulation buffer, latency bucket ladder,
//! percentile interpolation, window resolution, and the pure aggregation
//! functions the `/usage/*` handlers render (`DESIGN.md` §26).
//!
//! All SQL lives in `store.rs`; this module owns the maths and the buffer.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{AppError, Result};
use crate::models::{DailyUsageRow, ErrorTrend, LanguageDistribution, LatencyTrend, UsageQuery};

/// Inclusive upper bounds in milliseconds. Dense at the low end because NF01
/// targets p50 < 5 ms — a coarser ladder there would put p50 and p95 in the
/// same bucket and make both meaningless.
pub const LATENCY_BUCKETS_MS: [i64; 10] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000];

/// Sentinel `bucket_le_ms` for anything above the last finite boundary.
pub const BUCKET_OVERFLOW: i64 = -1;

/// How often the background task drains the buffer to the store. A crash
/// loses at most this much usage data — undercounting only, per F49.
// ponytail: 10s fixed; make it configurable only if a deployment actually
// needs a different durability/write-volume tradeoff.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(10);

/// F51 retention window.
pub const RETENTION_DAYS: i64 = 90;

/// F59 default window when no billing period applies.
pub const DEFAULT_WINDOW_DAYS: i64 = 30;

const SECONDS_PER_DAY: i64 = 86_400;

// ---- Buffer keys -------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DailyKey {
    pub day: String,
    pub tenant_id: String,
    pub language: String,
    pub status: i64,
    /// `AppError` slug; empty string for 2xx (F53 — no nullable columns).
    pub error_slug: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DailyCounters {
    pub request_count: i64,
    pub latency_sum_us: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LatencyKey {
    pub day: String,
    pub tenant_id: String,
    pub bucket_le_ms: i64,
}

// ---- Rows read back from the store ------------------------------------

/// One `usage_daily` group, already summed across tenants by the query.
#[derive(Debug, Clone)]
pub struct UsageDailyRow {
    pub day: String,
    pub language: String,
    pub status: i64,
    pub error_slug: String,
    pub request_count: i64,
    pub latency_sum_us: i64,
}

/// One `usage_latency` group, already summed across tenants by the query.
#[derive(Debug, Clone)]
pub struct UsageLatencyRow {
    pub day: String,
    pub bucket_le_ms: i64,
    pub request_count: i64,
}

// ---- Recorder ----------------------------------------------------------

/// Accumulates usage in memory so the request path never touches the
/// database. Drained by the background flush task (§26.4).
#[derive(Default)]
pub struct UsageRecorder {
    daily: Mutex<HashMap<DailyKey, DailyCounters>>,
    latency: Mutex<HashMap<LatencyKey, i64>>,
}

impl UsageRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called from the `record_usage` middleware. Lock-and-increment only —
    /// no I/O and no `await`, so it can neither fail nor slow the request
    /// that triggered it (F49).
    pub fn record(
        &self,
        tenant_id: &str,
        language: &str,
        status: u16,
        error_slug: &str,
        latency: Duration,
    ) {
        let day = format_day(epoch_day_now());

        {
            let mut daily = self.daily.lock().unwrap();
            let counters = daily
                .entry(DailyKey {
                    day: day.clone(),
                    tenant_id: tenant_id.to_string(),
                    language: language.to_string(),
                    status: status as i64,
                    error_slug: error_slug.to_string(),
                })
                .or_default();
            counters.request_count += 1;
            counters.latency_sum_us += latency.as_micros() as i64;
        }

        let mut latency_map = self.latency.lock().unwrap();
        *latency_map
            .entry(LatencyKey {
                day,
                tenant_id: tenant_id.to_string(),
                bucket_le_ms: bucket_for(latency),
            })
            .or_insert(0) += 1;
    }

    /// Empties both buffers, handing ownership to the caller for flushing.
    #[allow(clippy::type_complexity)]
    pub fn drain(&self) -> (Vec<(DailyKey, DailyCounters)>, Vec<(LatencyKey, i64)>) {
        let daily = std::mem::take(&mut *self.daily.lock().unwrap())
            .into_iter()
            .collect();
        let latency = std::mem::take(&mut *self.latency.lock().unwrap())
            .into_iter()
            .collect();
        (daily, latency)
    }
}

/// Ladder bucket a latency falls into, rounding up to the inclusive bound.
pub fn bucket_for(latency: Duration) -> i64 {
    let ms = latency.as_millis() as i64;
    LATENCY_BUCKETS_MS
        .iter()
        .copied()
        .find(|&bound| ms <= bound)
        .unwrap_or(BUCKET_OVERFLOW)
}

// ---- Percentiles -------------------------------------------------------

/// Linear interpolation inside the bucket that crosses the target rank, the
/// same approach as Prometheus' `histogram_quantile`. Returns `None` for an
/// empty histogram. A rank landing in the overflow bucket reports the last
/// finite boundary rather than extrapolating into an unbounded range — the
/// only honest answer available.
pub fn percentile(buckets: &[(i64, i64)], q: f64) -> Option<u64> {
    let total: i64 = buckets.iter().map(|(_, c)| *c).sum();
    if total <= 0 {
        return None;
    }

    let mut ordered: Vec<(i64, i64)> = buckets
        .iter()
        .copied()
        .filter(|(bound, _)| *bound != BUCKET_OVERFLOW)
        .collect();
    ordered.sort_by_key(|(bound, _)| *bound);
    let overflow: i64 = buckets
        .iter()
        .filter(|(bound, _)| *bound == BUCKET_OVERFLOW)
        .map(|(_, c)| *c)
        .sum();

    let last_finite = LATENCY_BUCKETS_MS[LATENCY_BUCKETS_MS.len() - 1];
    let rank = q * total as f64;

    let mut cumulative = 0i64;
    let mut lower = 0i64;
    for (upper, count) in ordered {
        if count <= 0 {
            lower = upper;
            continue;
        }
        let next = cumulative + count;
        if (next as f64) >= rank {
            let into_bucket = rank - cumulative as f64;
            let fraction = (into_bucket / count as f64).clamp(0.0, 1.0);
            let value = lower as f64 + (upper - lower) as f64 * fraction;
            return Some(value.round() as u64);
        }
        cumulative = next;
        lower = upper;
    }

    if overflow > 0 {
        return Some(last_finite as u64);
    }
    Some(lower.max(0) as u64)
}

// ---- Aggregation (pure; the handlers only render these) -----------------

/// `/usage/daily` is always per-date — an undated "daily" aggregate is
/// meaningless (F58's carve-out).
pub fn aggregate_daily(rows: &[UsageDailyRow]) -> Vec<DailyUsageRow> {
    let mut by_day: HashMap<&str, (i64, i64, i64)> = HashMap::new();
    for row in rows {
        let entry = by_day.entry(row.day.as_str()).or_insert((0, 0, 0));
        entry.0 += row.request_count;
        entry.1 += row.latency_sum_us;
        if row.status >= 400 {
            entry.2 += row.request_count;
        }
    }

    let mut out: Vec<DailyUsageRow> = by_day
        .into_iter()
        .map(|(day, (requests, latency_sum_us, errors))| DailyUsageRow {
            date: day.to_string(),
            requests: requests as u64,
            average_latency_ms: if requests > 0 {
                (latency_sum_us / requests / 1_000) as u64
            } else {
                0
            },
            errors: errors as u64,
        })
        .collect();
    out.sort_by(|a, b| a.date.cmp(&b.date));
    out
}

/// `/usage/errors`, carrying both dimensions per F56.
pub fn aggregate_errors(rows: &[UsageDailyRow], dated: bool) -> Vec<ErrorTrend> {
    let mut grouped: HashMap<(Option<String>, i64, String), i64> = HashMap::new();
    for row in rows.iter().filter(|r| r.status >= 400) {
        let date = dated.then(|| row.day.clone());
        *grouped
            .entry((date, row.status, row.error_slug.clone()))
            .or_insert(0) += row.request_count;
    }

    let mut out: Vec<ErrorTrend> = grouped
        .into_iter()
        .map(|((date, status, error_code), count)| ErrorTrend {
            date,
            status: status as u16,
            error_code,
            count: count as u64,
        })
        .collect();
    out.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then(b.count.cmp(&a.count))
            .then(a.error_code.cmp(&b.error_code))
    });
    out
}

/// `/usage/languages`. Percentages are computed against the same
/// scope-filtered rows, so an admin caller can never be divided by a
/// platform-wide total (F61).
pub fn aggregate_languages(rows: &[UsageDailyRow], dated: bool) -> Vec<LanguageDistribution> {
    let mut grouped: HashMap<(Option<String>, String), i64> = HashMap::new();
    let mut totals: HashMap<Option<String>, i64> = HashMap::new();
    for row in rows {
        let date = dated.then(|| row.day.clone());
        *grouped
            .entry((date.clone(), row.language.clone()))
            .or_insert(0) += row.request_count;
        *totals.entry(date).or_insert(0) += row.request_count;
    }

    let mut out: Vec<LanguageDistribution> = grouped
        .into_iter()
        .map(|((date, language), count)| {
            let total = totals.get(&date).copied().unwrap_or(0);
            LanguageDistribution {
                date,
                language,
                count: count as u64,
                // `total` is the sum this count contributed to, so it is only
                // zero when `count` is too — no division by zero possible.
                percentage: if total > 0 {
                    ((count as f64 / total as f64) * 1_000.0).round() / 10.0
                } else {
                    0.0
                },
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then(b.count.cmp(&a.count))
            .then(a.language.cmp(&b.language))
    });
    out
}

/// `/usage/latency`. Buckets are additive, so a multi-day aggregate is a
/// valid histogram rather than an average of averages.
pub fn aggregate_latency(rows: &[UsageLatencyRow], dated: bool) -> Vec<LatencyTrend> {
    let mut grouped: HashMap<Option<String>, Vec<(i64, i64)>> = HashMap::new();
    for row in rows {
        let date = dated.then(|| row.day.clone());
        grouped
            .entry(date)
            .or_default()
            .push((row.bucket_le_ms, row.request_count));
    }

    let mut dates: Vec<Option<String>> = grouped.keys().cloned().collect();
    dates.sort();

    let mut out = Vec::new();
    for date in dates {
        let buckets = &grouped[&date];
        for (label, q) in [("p50", 0.50), ("p95", 0.95), ("p99", 0.99)] {
            if let Some(value_ms) = percentile(buckets, q) {
                out.push(LatencyTrend {
                    date: date.clone(),
                    percentile: label.to_string(),
                    value_ms,
                });
            }
        }
    }
    out
}

// ---- Window resolution -------------------------------------------------

/// An inclusive `YYYY-MM-DD` range. `dated` records whether the caller
/// supplied an explicit window, which selects between F58's two shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub start: String,
    pub end: String,
    pub dated: bool,
}

/// `period` is the calling tenant's billing period (F59), if it has one.
/// Platform callers pass `None` and get the rolling default window.
pub fn resolve_window(query: &UsageQuery, period: Option<(u64, u64)>) -> Result<Window> {
    let today = epoch_day_now();

    match (query.start.as_deref(), query.end.as_deref()) {
        (Some(start), Some(end)) => {
            let start_day = parse_day(start).ok_or_else(|| {
                AppError::InvalidDateRange(format!("invalid start date: {start}"))
            })?;
            let end_day = parse_day(end)
                .ok_or_else(|| AppError::InvalidDateRange(format!("invalid end date: {end}")))?;
            if start_day > end_day {
                return Err(AppError::InvalidDateRange(
                    "start must not be after end".to_string(),
                ));
            }
            if end_day - start_day >= RETENTION_DAYS {
                return Err(AppError::InvalidDateRange(format!(
                    "range must span fewer than {RETENTION_DAYS} days of retained data"
                )));
            }
            Ok(Window {
                start: format_day(start_day),
                end: format_day(end_day),
                dated: true,
            })
        }
        // A half-open window would silently mean different things per scope.
        (Some(_), None) | (None, Some(_)) => Err(AppError::InvalidDateRange(
            "start and end must be supplied together".to_string(),
        )),
        (None, None) => {
            // An un-provisioned tenant (no billing period set) still deserves
            // to see its data, so fall back rather than erroring.
            let (start_day, end_day) = match period {
                Some((start_ts, end_ts)) => (
                    (start_ts as i64) / SECONDS_PER_DAY,
                    (end_ts as i64) / SECONDS_PER_DAY,
                ),
                None => (today - DEFAULT_WINDOW_DAYS + 1, today),
            };
            Ok(Window {
                start: format_day(start_day),
                end: format_day(end_day),
                dated: false,
            })
        }
    }
}

/// Oldest day that survives the F51 purge.
pub fn retention_cutoff_day() -> String {
    format_day(epoch_day_now() - RETENTION_DAYS)
}

// ---- Civil date helpers ------------------------------------------------
//
// `YYYY-MM-DD` sorts lexicographically, so the store compares days as plain
// strings and needs no date column type. Only the epoch-day <-> civil-date
// conversion is required, which is small enough not to justify pulling in a
// date crate (`chrono`/`time` are not otherwise in the dependency tree).

pub fn epoch_day_now() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64;
    secs / SECONDS_PER_DAY
}

/// Days since the Unix epoch -> `YYYY-MM-DD` (Howard Hinnant's
/// `civil_from_days`, proleptic Gregorian).
pub fn format_day(epoch_day: i64) -> String {
    let z = epoch_day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD` -> days since the Unix epoch, rejecting malformed input and
/// impossible dates (Hinnant's `days_from_civil`).
pub fn parse_day(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }

    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        day: &str,
        language: &str,
        status: i64,
        slug: &str,
        count: i64,
        sum_us: i64,
    ) -> UsageDailyRow {
        UsageDailyRow {
            day: day.to_string(),
            language: language.to_string(),
            status,
            error_slug: slug.to_string(),
            request_count: count,
            latency_sum_us: sum_us,
        }
    }

    #[test]
    fn civil_date_roundtrips_across_epoch_and_leap_years() {
        for day in [-25_567i64, 0, 1, 19_000, 20_301, 25_000] {
            assert_eq!(parse_day(&format_day(day)), Some(day), "day {day}");
        }
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(parse_day("2024-02-29"), Some(19_782));
        assert_eq!(format_day(19_782), "2024-02-29");
    }

    #[test]
    fn parse_day_rejects_malformed_and_impossible_dates() {
        for bad in [
            "2026-13-01",
            "2026-00-10",
            "2026-02-30",
            "2025-02-29", // not a leap year
            "2026-1-01",
            "20260101",
            "",
            "not-a-date",
        ] {
            assert!(parse_day(bad).is_none(), "should reject {bad}");
        }
    }

    #[test]
    fn bucket_for_rounds_up_to_inclusive_bound() {
        assert_eq!(bucket_for(Duration::from_micros(500)), 1);
        assert_eq!(bucket_for(Duration::from_millis(1)), 1);
        assert_eq!(bucket_for(Duration::from_millis(3)), 5);
        assert_eq!(bucket_for(Duration::from_millis(1_000)), 1_000);
        assert_eq!(bucket_for(Duration::from_millis(1_001)), BUCKET_OVERFLOW);
    }

    #[test]
    fn percentile_interpolates_within_the_crossing_bucket() {
        // 100 requests, all in the 5..=10 ms bucket: p50 lands halfway.
        let buckets = [(1, 0), (2, 0), (5, 0), (10, 100)];
        assert_eq!(percentile(&buckets, 0.50), Some(8));

        // Half at <=1 ms, half at <=100 ms. Rank 99 of 100 sits 98% of the
        // way into the second bucket: 1 + (100 - 1) * 0.98 = 98.02.
        let buckets = [(1, 50), (100, 50)];
        assert_eq!(percentile(&buckets, 0.50), Some(1));
        assert_eq!(percentile(&buckets, 0.99), Some(98));
    }

    #[test]
    fn percentile_clamps_overflow_to_last_finite_boundary() {
        let buckets = [(1, 10), (BUCKET_OVERFLOW, 90)];
        assert_eq!(percentile(&buckets, 0.99), Some(1_000));
    }

    #[test]
    fn percentile_of_empty_histogram_is_none() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[(1, 0), (10, 0)], 0.5), None);
    }

    #[test]
    fn recorder_accumulates_and_drains_once() {
        let recorder = UsageRecorder::new();
        recorder.record("t1", "en_US", 200, "", Duration::from_millis(3));
        recorder.record("t1", "en_US", 200, "", Duration::from_millis(4));
        recorder.record(
            "t1",
            "en_US",
            400,
            "validation-error",
            Duration::from_millis(1),
        );

        let (daily, latency) = recorder.drain();
        assert_eq!(daily.len(), 2, "one row per (status, slug) pair");
        let ok = daily
            .iter()
            .find(|(k, _)| k.status == 200)
            .expect("200 row present");
        assert_eq!(ok.1.request_count, 2);
        assert_eq!(ok.1.latency_sum_us, 7_000);

        let total: i64 = latency.iter().map(|(_, c)| *c).sum();
        assert_eq!(total, 3);

        // Draining empties the buffer, so a second flush writes nothing.
        let (daily, latency) = recorder.drain();
        assert!(daily.is_empty() && latency.is_empty());
    }

    #[test]
    fn aggregate_daily_sums_per_day_and_counts_only_4xx_5xx_as_errors() {
        let rows = vec![
            row("2026-07-30", "en_US", 200, "", 8, 16_000),
            row("2026-07-30", "en_US", 400, "validation-error", 2, 2_000),
            row("2026-07-31", "en_US", 200, "", 5, 10_000),
        ];
        let out = aggregate_daily(&rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].date, "2026-07-30");
        assert_eq!(out[0].requests, 10);
        assert_eq!(out[0].errors, 2);
        assert_eq!(out[0].average_latency_ms, 1); // 18_000us / 10 / 1000
        assert_eq!(out[1].errors, 0);
    }

    #[test]
    fn aggregate_errors_excludes_success_and_keeps_both_dimensions() {
        let rows = vec![
            row("2026-07-30", "en_US", 200, "", 100, 0),
            row("2026-07-30", "en_US", 400, "validation-error", 3, 0),
            row("2026-07-31", "en_US", 400, "validation-error", 4, 0),
        ];
        let flat = aggregate_errors(&rows, false);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].count, 7);
        assert_eq!(flat[0].status, 400);
        assert_eq!(flat[0].error_code, "validation-error");
        assert!(flat[0].date.is_none());

        let dated = aggregate_errors(&rows, true);
        assert_eq!(dated.len(), 2);
        assert_eq!(dated[0].date.as_deref(), Some("2026-07-30"));
    }

    #[test]
    fn aggregate_languages_percentages_use_the_filtered_total() {
        let rows = vec![
            row("2026-07-30", "en_US", 200, "", 800, 0),
            row("2026-07-30", "es", 200, "", 200, 0),
        ];
        let out = aggregate_languages(&rows, false);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].language, "en_US");
        assert_eq!(out[0].percentage, 80.0);
        assert_eq!(out[1].percentage, 20.0);
        let sum: f64 = out.iter().map(|l| l.percentage).sum();
        assert!((sum - 100.0).abs() < 0.05);
    }

    #[test]
    fn aggregate_latency_merges_buckets_across_days_when_flat() {
        let rows = vec![
            UsageLatencyRow {
                day: "2026-07-30".to_string(),
                bucket_le_ms: 10,
                request_count: 50,
            },
            UsageLatencyRow {
                day: "2026-07-31".to_string(),
                bucket_le_ms: 10,
                request_count: 50,
            },
        ];
        let flat = aggregate_latency(&rows, false);
        assert_eq!(flat.len(), 3, "p50/p95/p99");
        assert!(flat.iter().all(|t| t.date.is_none()));

        let dated = aggregate_latency(&rows, true);
        assert_eq!(dated.len(), 6, "three percentiles per day");
    }

    #[test]
    fn resolve_window_rejects_inverted_partial_and_oversized_ranges() {
        let inverted = UsageQuery {
            start: Some("2026-07-31".to_string()),
            end: Some("2026-07-01".to_string()),
        };
        assert!(resolve_window(&inverted, None).is_err());

        let partial = UsageQuery {
            start: Some("2026-07-01".to_string()),
            end: None,
        };
        assert!(resolve_window(&partial, None).is_err());

        let oversized = UsageQuery {
            start: Some("2020-01-01".to_string()),
            end: Some("2026-01-01".to_string()),
        };
        assert!(resolve_window(&oversized, None).is_err());

        let malformed = UsageQuery {
            start: Some("2026-02-30".to_string()),
            end: Some("2026-03-01".to_string()),
        };
        assert!(resolve_window(&malformed, None).is_err());
    }

    #[test]
    fn resolve_window_defaults_to_billing_period_then_falls_back() {
        let empty = UsageQuery {
            start: None,
            end: None,
        };

        // Billing period supplied (F59, admin scope).
        let period = Some((19_000 * 86_400u64, 19_030 * 86_400u64));
        let window = resolve_window(&empty, period).unwrap();
        assert_eq!(window.start, format_day(19_000));
        assert_eq!(window.end, format_day(19_030));
        assert!(!window.dated);

        // No period (platform scope, or an un-provisioned tenant) -> 30 days.
        let window = resolve_window(&empty, None).unwrap();
        let today = epoch_day_now();
        assert_eq!(window.end, format_day(today));
        assert_eq!(window.start, format_day(today - DEFAULT_WINDOW_DAYS + 1));
        assert!(!window.dated);
    }

    #[test]
    fn resolve_window_marks_explicit_ranges_as_dated() {
        let query = UsageQuery {
            start: Some("2026-07-01".to_string()),
            end: Some("2026-07-31".to_string()),
        };
        let window = resolve_window(&query, None).unwrap();
        assert_eq!(window.start, "2026-07-01");
        assert_eq!(window.end, "2026-07-31");
        assert!(window.dated);
    }
}
