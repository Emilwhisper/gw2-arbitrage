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

use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Sell velocity per trailing window, in units/day.
#[derive(Debug, Clone, Copy, Default)]
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
    let client = reqwest::Client::new();
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
