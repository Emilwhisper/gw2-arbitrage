//! Sell-velocity estimates from the community datawars2.ie TP history API.
//!
//! Velocity = units sold per day (instant-buys off the sell wall), computed
//! over several trailing windows. `sell_delisted` (cancellations) is
//! deliberately ignored.
//!
//! Endpoints (no auth):
//! - `https://api.datawars2.ie/gw2/v2/history/hourly/json?itemID=X` (1h buckets)
//! - `https://api.datawars2.ie/gw2/v2/history/json?itemID=X&start=YYYY-MM-DD` (1d buckets)
//!
//! Multi-ID requests are NOT supported (verified empirically), so this module
//! fetches one item at a time; the GUI batches calls across worker threads.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Shared HTTP client. Reusing a single client keeps the connection pool
/// (keep-alive) alive across requests and avoids rebuilding TLS state for every
/// call, which matters when many worker threads fetch concurrently.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client")
});

/// Sell velocity per trailing window, in units/day.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Velocity {
    pub h6: Option<f64>,
    pub h12: Option<f64>,
    pub h24: Option<f64>,
    pub d7: Option<f64>,
    pub w2: Option<f64>,
    pub m1: Option<f64>,
    pub m3: Option<f64>,
    pub m6: Option<f64>,
    pub y1: Option<f64>,
    pub y2: Option<f64>,
}

#[derive(Deserialize)]
struct Bucket {
    date: String,
    sell_sold: f64,
}

/// How long cached velocity data stays fresh (24 hours).
pub const CACHE_TTL_SECS: i64 = 24 * 60 * 60;

/// On-disk velocity cache entry, one file per item.
#[derive(Serialize, Deserialize)]
struct CachedVelocity {
    fetched_at: i64,
    /// which endpoint groups the cached values cover
    hourly: bool,
    daily: bool,
    velocity: Velocity,
}

/// Path of the per-item velocity cache file (`velocity_<item_id>.json`).
pub fn cache_path(cache_dir: &Path, item_id: u32) -> PathBuf {
    cache_dir.join(format!("velocity_{}.json", item_id))
}

/// Load a fresh (within `CACHE_TTL_SECS`) cache entry, if any.
fn load_cache(cache_dir: &Path, item_id: u32) -> Option<CachedVelocity> {
    let text = std::fs::read_to_string(cache_path(cache_dir, item_id)).ok()?;
    let cached: CachedVelocity = serde_json::from_str(&text).ok()?;
    if now_unix() - cached.fetched_at > CACHE_TTL_SECS {
        return None;
    }
    Some(cached)
}

/// Persist velocity values for an item.
fn save_cache(cache_dir: &Path, item_id: u32, v: &Velocity, hourly: bool, daily: bool) {
    let cached = CachedVelocity {
        fetched_at: now_unix(),
        hourly,
        daily,
        velocity: *v,
    };
    let Ok(text) = serde_json::to_string(&cached) else {
        return;
    };
    let path = cache_path(cache_dir, item_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

const BASE_URL: &str = "https://api.datawars2.ie/gw2/v2/history";

/// Require at least 80% of the expected buckets, else treat as low confidence.
fn enough_coverage(actual: usize, expected: usize) -> bool {
    (actual as f64) >= 0.8 * (expected as f64) && actual > 0
}

/// Seconds since epoch parsed from "YYYY-MM-DDTHH:MM:SSZ" or "YYYY-MM-DD".
/// Manual parsing avoids a chrono dependency.
fn parse_epoch(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: i64 = date.get(5..7)?.parse().ok()?;
    let d: i64 = date.get(8..10)?.parse().ok()?;
    let h: i64 = if b.len() >= 13 {
        date.get(11..13)?.parse().ok()?
    } else {
        0
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days from civil date (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// ISO date (YYYY-MM-DD) for `now_unix - days_back * 86400`.
fn iso_date(days_back: i64) -> String {
    let days = now_unix() / 86400 - days_back;
    // civil_from_days (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Fetch sell velocity for one item. Units: items/day.
/// `fetch_hourly` / `fetch_daily` let the caller skip a whole endpoint when
/// every window that uses it is disabled in the GUI.
pub async fn fetch_velocity(
    item_id: u32,
    fetch_hourly: bool,
    fetch_daily: bool,
) -> Result<Velocity, String> {
    let client = &*CLIENT;
    let now = now_unix();
    let mut v = Velocity::default();

    // --- hourly buckets: 6h / 12h / 24h windows ---
    if fetch_hourly {
        let url = format!("{}/hourly/json?itemID={}", BASE_URL, item_id);
        let rows: Vec<Bucket> = client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        for (hours, out) in [
            (6, 0usize),
            (12, 1),
            (24, 2),
        ] {
            let cutoff = now - hours * 3600;
            let buckets: Vec<&Bucket> = rows
                .iter()
                .filter(|b| parse_epoch(&b.date).is_some_and(|t| t >= cutoff))
                .collect();
            if enough_coverage(buckets.len(), hours as usize) {
                let sum: f64 = buckets.iter().map(|b| b.sell_sold).sum();
                let value = sum / buckets.len() as f64 * 24.0;
                match out {
                    0 => v.h6 = Some(value),
                    1 => v.h12 = Some(value),
                    _ => v.h24 = Some(value),
                }
            }
        }
    }

    // --- daily buckets: 7d / 2w / 1m / 3m / 6m / 1y / 2y windows ---
    if fetch_daily {
        let url = format!(
            "{}/json?itemID={}&start={}",
            BASE_URL,
            item_id,
            iso_date(735)
        );
        let rows: Vec<Bucket> = client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        for (days, out) in [
            (7, 0usize),
            (14, 1),
            (30, 2),
            (90, 3),
            (180, 4),
            (365, 5),
            (730, 6),
        ] {
            let cutoff = now - days * 86400;
            let buckets: Vec<i64> = rows
                .iter()
                .filter_map(|b| parse_epoch(&b.date))
                .filter(|&t| t >= cutoff)
                .collect();
            let span_days = buckets.iter().max().and_then(|&max| {
                buckets
                    .iter()
                    .min()
                    .map(|&min| ((max - min) / 86400 + 1).max(1))
            });
            if let Some(span_days) = span_days {
                if (span_days as f64) >= 0.8 * (days as f64) {
                    let sum: f64 = rows
                        .iter()
                        .filter(|b| parse_epoch(&b.date).is_some_and(|t| t >= cutoff))
                        .map(|b| b.sell_sold)
                        .sum();
                    let value = sum / span_days as f64;
                    match out {
                        0 => v.d7 = Some(value),
                        1 => v.w2 = Some(value),
                        2 => v.m1 = Some(value),
                        3 => v.m3 = Some(value),
                        4 => v.m6 = Some(value),
                        5 => v.y1 = Some(value),
                        _ => v.y2 = Some(value),
                    }
                }
            }
        }
    }

    Ok(v)
}

/// Fetch sell velocity, reusing the on-disk cache when it is still fresh
/// (`CACHE_TTL_SECS`). Returns the velocity plus which endpoint groups the
/// returned values cover, so the caller can merge them with groups it already
/// has (and remember what does not need fetching again).
///
/// `fetch_hourly` / `fetch_daily` are requests, not demands: a group that is
/// already present in a fresh cache entry is not downloaded again. On a network
/// failure the cached values are used when available.
pub async fn fetch_velocity_cached(
    cache_dir: &Path,
    item_id: u32,
    fetch_hourly: bool,
    fetch_daily: bool,
) -> Result<(Velocity, bool, bool), String> {
    let cached = load_cache(cache_dir, item_id);
    let have_hourly = cached.as_ref().is_some_and(|c| c.hourly);
    let have_daily = cached.as_ref().is_some_and(|c| c.daily);
    let cached_velocity = cached.as_ref().map(|c| c.velocity);
    let need_hourly = fetch_hourly && !have_hourly;
    let need_daily = fetch_daily && !have_daily;
    let out_hourly = have_hourly || need_hourly;
    let out_daily = have_daily || need_daily;

    if !need_hourly && !need_daily {
        return match cached {
            Some(c) => Ok((c.velocity, out_hourly, out_daily)),
            None => Ok((Velocity::default(), false, false)),
        };
    }

    match fetch_velocity(item_id, need_hourly, need_daily).await {
        Ok(fresh) => {
            let mut merged = cached_velocity.unwrap_or_default();
            if need_hourly {
                merged.h6 = fresh.h6;
                merged.h12 = fresh.h12;
                merged.h24 = fresh.h24;
            }
            if need_daily {
                merged.d7 = fresh.d7;
                merged.w2 = fresh.w2;
                merged.m1 = fresh.m1;
                merged.m3 = fresh.m3;
                merged.m6 = fresh.m6;
                merged.y1 = fresh.y1;
                merged.y2 = fresh.y2;
            }
            save_cache(cache_dir, item_id, &merged, out_hourly, out_daily);
            Ok((merged, out_hourly, out_daily))
        }
        Err(e) => match cached_velocity {
            Some(v) => Ok((v, out_hourly, out_daily)),
            None => Err(e),
        },
    }
}
