use crate::api::ItemListings;

use bincode;
use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use serde_json;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::File;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use once_cell::sync::Lazy;

use crate::config;

const PARALLEL_REQUESTS: usize = 10;
const MAX_PAGE_SIZE: i32 = 200; // https://wiki.guildwars2.com/wiki/API:2#Paging
const MAX_ITEM_ID_LENGTH: i32 = 200; // error returned for greater than this amount

/// How many times a single URL is attempted before giving up.
const MAX_ATTEMPTS: u32 = 4;

/// Shared HTTP client: connection reuse (no TLS handshake per request) and a
/// per-request timeout so a hung connection can't stall a scan forever.
///
/// A truthful app User-Agent is set: besides identifying us, it (unlike the
/// default library one) makes the API actually send gzip-compressed responses
/// (~10x smaller bodies), which the `gzip` reqwest feature then decodes
/// transparently.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!(
            "gw2-arbitrage/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/Emilwhisper/gw2-arbitrage)"
        ))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client")
});

/// Outcome of one HTTP attempt: a value, a transient failure worth retrying
/// (throttling, 5xx, CDN error pages), or a fatal one.
enum Attempt<T> {
    Ok(T),
    Retry {
        detail: String,
        retry_after_secs: Option<u64>,
    },
    Fatal(String),
}

pub async fn fetch_item_listings(
    item_ids: &[u32],
    cache_dir: Option<&PathBuf>,
    notify: Option<&dyn Fn(&str)>,
) -> Result<Vec<ItemListings>, Box<dyn std::error::Error>> {
    let mut tp_listings: Vec<ItemListings> =
        request_item_ids("commerce/listings", item_ids, cache_dir, notify, false).await?;

    for listings in &mut tp_listings {
        // by default sells are listed in ascending and buys in descending price.
        // reverse lists to allow best offers to be popped instead of spliced from front.
        listings.buys.reverse();
        listings.sells.reverse();
    }

    Ok(tp_listings)
}

pub async fn get_data<T, Fut>(
    data_path: impl AsRef<Path>,
    getter: impl FnOnce() -> Fut,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
    Fut: Future<Output = Result<Vec<T>, Box<dyn std::error::Error>>>,
{
    if let Ok(file) = File::open(&data_path) {
        let stream = DeflateDecoder::new(file);
        deserialize_from(stream).map_err(|e| {
            format!(
                "Failed to deserialize existing data at '{}' ({}). \
                 Try using the --reset-data flag to replace the data files.",
                data_path.as_ref().display(),
                e,
            )
            .into()
        })
    } else {
        let items = getter().await?;

        let file = File::create(data_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &items)?;

        Ok(items)
    }
}

pub async fn request_paginated<T>(
    url_path: &str,
    lang: &Option<config::Language>,
    notify: Option<&dyn Fn(&str)>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let mut page_no = 0;
    let mut page_total = None;

    // update page total with first request
    let mut items: Vec<T> = request_page(url_path, page_no, &mut page_total, lang, notify).await?;

    // fetch remaining pages in parallel batches
    page_no += 1;

    // try fetching one extra page in case page total increased while paginating
    let page_total = page_total.expect("Missing page total") + 1;

    let request_results = stream::iter((page_no..page_total).map(|page_no| async move {
        request_page::<T>(url_path, page_no, &mut Some(page_total), lang, notify).await
    }))
    .buffered(PARALLEL_REQUESTS)
    .collect::<Vec<Result<Vec<T>, Box<dyn std::error::Error>>>>()
    .await;

    for result in request_results.into_iter() {
        let mut new_items = result?;
        items.append(&mut new_items);
    }

    Ok(items)
}

async fn request_page<T>(
    url_path: &str,
    page_no: usize,
    page_total: &mut Option<usize>,
    lang: &Option<config::Language>,
    notify: Option<&dyn Fn(&str)>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let url = if let Some(code) = config::Language::code(lang) {
        format!(
            "https://api.guildwars2.com/v2/{}?lang={}&page={}&page_size={}",
            url_path, code, page_no, MAX_PAGE_SIZE
        )
    } else {
        format!(
            "https://api.guildwars2.com/v2/{}?page={}&page_size={}",
            url_path, page_no, MAX_PAGE_SIZE
        )
    };

    if let Some(notify) = notify {
        // page_total is known for every page but the first: show progress
        match *page_total {
            Some(total) => notify(&format!("{} [{}/{}]", url, page_no + 1, total)),
            None => notify(&url),
        }
    }

    let mut last_detail = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let response = match CLIENT.get(&url).send().await {
            Ok(response) => response,
            Err(e) => {
                // transport error (timeout, reset, DNS): transient
                last_detail = format!("request error: {}", e);
                if attempt < MAX_ATTEMPTS {
                    backoff_sleep(attempt, None).await;
                    continue;
                }
                break;
            }
        };
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            last_detail = format!("HTTP {}", status);
            if attempt < MAX_ATTEMPTS {
                backoff_sleep(attempt, retry_after_secs(&response)).await;
                continue;
            }
            break;
        }
        if page_total.is_none() {
            match response
                .headers()
                .get("X-Page-Total")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok())
            {
                Some(total) => *page_total = Some(total),
                // missing/garbled header (e.g. a CDN error page): transient
                None => {
                    last_detail =
                        format!("missing X-Page-Total header (HTTP {})", response.status());
                    if attempt < MAX_ATTEMPTS {
                        backoff_sleep(attempt, retry_after_secs(&response)).await;
                        continue;
                    }
                    break;
                }
            }
        }

        let txt = response.text().await.unwrap_or_default();
        if txt.contains("page out of range") {
            return Ok(vec![]);
        }
        let de = &mut serde_json::Deserializer::from_str(&txt);
        match serde_path_to_error::deserialize(de) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_detail = format!(
                    "invalid JSON ({} bytes): {} (body: {})",
                    txt.len(),
                    e,
                    truncate(&txt, 300)
                );
                // HTML/empty bodies on a 200 are CDN error pages: retryable.
                // JSON-shaped bodies that don't match our types are a real
                // schema mismatch: fail fast with the context above.
                let retryable = txt.trim_start().starts_with('<') || txt.trim().is_empty();
                if retryable && attempt < MAX_ATTEMPTS {
                    backoff_sleep(attempt, None).await;
                    continue;
                }
                break;
            }
        }
    }
    Err(format!(
        "GET {} failed after {} attempts: {}",
        redact_url(&url),
        MAX_ATTEMPTS,
        last_detail
    )
    .into())
}

pub async fn request_item_ids<T>(
    url_path: &str,
    item_ids: &[u32],
    cache_dir: Option<&PathBuf>,
    notify: Option<&dyn Fn(&str)>,
    // when true, batches the API rejects with "all ids provided are invalid"
    // contribute nothing instead of failing the whole call (targeted fetches
    // over id sets that partly lack market data; missing entries simply mean
    // "unobtainable here", like everywhere else in the estimate)
    tolerate_invalid: bool,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    // fetch batches in parallel (`buffered` preserves batch order, so the
    // concatenated result is identical to the old sequential loop)
    let batches: Vec<_> = item_ids.chunks(MAX_ITEM_ID_LENGTH as usize).collect();
    let total_batches = batches.len();
    let batch_results = stream::iter(batches.into_iter().enumerate().map(
        |(batch_no, batch)| async move {
            let item_ids_str: Vec<String> = batch.iter().map(|id| id.to_string()).collect();
            let url = format!(
                "https://api.guildwars2.com/v2/{}?ids={}",
                url_path,
                item_ids_str.join(",")
            );
            if let Some(notify) = notify.filter(|_| total_batches > 1) {
                notify(&format!(
                    "{} [batch {}/{}]",
                    url,
                    batch_no + 1,
                    total_batches
                ));
            }
            if let Some(cache_dir) = cache_dir {
                cached_fetch::<Vec<T>>(&url, cache_dir, notify).await
            } else {
                fetch::<Vec<T>>(&url, None).await
            }
        },
    ))
    .buffered(PARALLEL_REQUESTS)
    .collect::<Vec<Result<Vec<T>, Box<dyn std::error::Error>>>>()
    .await;

    let mut result = vec![];
    for batch_result in batch_results.into_iter() {
        match batch_result {
            Ok(batch) => result.extend(batch.into_iter()),
            Err(e)
                if tolerate_invalid && e.to_string().contains("all ids provided are invalid") =>
            {
                // a whole 200-id batch without market data: nothing to add
            }
            Err(e) => return Err(e),
        }
    }

    Ok(result)
}

pub async fn fetch_account_recipes(
    key: &str,
    cache_dir: &PathBuf,
    notify: Option<&dyn Fn(&str)>,
) -> Result<HashSet<u32>, Box<dyn std::error::Error>> {
    let base = "https://api.guildwars2.com/v2/account/recipes?access_token=";
    let url = format!("{}{}", base, key);
    if let Some(notify) = notify {
        let display = format!("{}{}", base, "<api-key>");
        let private = |_url: &str| notify(&display);
        Ok(cached_fetch(&url, cache_dir, Some(&private as &dyn Fn(&str))).await?)
    } else {
        Ok(cached_fetch(&url, cache_dir, None).await?)
    }
}

async fn cached_fetch<T>(
    url: &str,
    cache_dir: &Path,
    notify: Option<&dyn Fn(&str)>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let cache_path = url_to_cache_path(url, cache_dir);
    if let Ok(file) = File::open(&cache_path) {
        let stream = DeflateDecoder::new(file);
        let v = deserialize_from(stream)?;
        return Ok(v);
    }

    let v = fetch(&url, notify).await?;

    // save cache file
    let file = File::create(cache_path)?;
    let stream = DeflateEncoder::new(file, Compression::default());
    serialize_into(stream, &v)?;

    Ok(v)
}

async fn fetch<T>(url: &str, notify: Option<&dyn Fn(&str)>) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    if let Some(notify) = notify {
        notify(&url.to_string());
    }

    let mut last_detail = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        match try_fetch::<T>(url).await {
            Attempt::Ok(v) => return Ok(v),
            Attempt::Retry {
                detail,
                retry_after_secs,
            } => {
                last_detail = detail;
                if attempt < MAX_ATTEMPTS {
                    backoff_sleep(attempt, retry_after_secs).await;
                    continue;
                }
            }
            Attempt::Fatal(detail) => {
                return Err(format!("GET {}: {}", redact_url(url), detail).into());
            }
        }
    }
    Err(format!(
        "GET {} failed after {} attempts: {}",
        redact_url(url),
        MAX_ATTEMPTS,
        last_detail
    )
    .into())
}

/// One GET + JSON-parse attempt. Transient CDN/API hiccups (transport errors,
/// 429/5xx, empty or HTML bodies on a 200) come back retryable; everything
/// else (400/404, JSON-shaped but unexpected bodies) is fatal.
async fn try_fetch<T>(url: &str) -> Attempt<T>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let response = match CLIENT.get(url).send().await {
        Ok(response) => response,
        Err(e) => {
            return Attempt::Retry {
                detail: format!("request error: {}", e),
                retry_after_secs: None,
            }
        }
    };
    let status = response.status();
    let retry_after_secs = retry_after_secs(&response);
    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Attempt::Retry {
            detail: format!("HTTP {} (body: {})", status, truncate(&body, 200)),
            retry_after_secs,
        };
    }
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|err| {
                err.get("text")
                    .and_then(|text| text.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| truncate(&body, 300));
        return Attempt::Fatal(format!("HTTP {}: {}", status, detail));
    }

    let de = &mut serde_json::Deserializer::from_str(&body);
    match serde_path_to_error::deserialize(de) {
        Ok(v) => Attempt::Ok(v),
        Err(e) => {
            let detail = format!(
                "invalid JSON ({} bytes): {} (body: {})",
                body.len(),
                e,
                truncate(&body, 300)
            );
            // HTML/empty bodies on a 200 are CDN error pages (e.g. a 504
            // Gateway Time-out served as 200): retryable. JSON-shaped bodies
            // that don't match our types are a real schema mismatch: fatal.
            if body.trim_start().starts_with('<') || body.trim().is_empty() {
                Attempt::Retry {
                    detail,
                    retry_after_secs: None,
                }
            } else {
                Attempt::Fatal(detail)
            }
        }
    }
}

/// Best-effort `Retry-After` (seconds) from a response, if the server sent one.
fn retry_after_secs(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

/// Exponential backoff (1s, 2s, 4s, ...), honoring the server's Retry-After
/// when it asks for longer (capped so a scan can't stall forever).
async fn backoff_sleep(failed_attempt: u32, retry_after_secs: Option<u64>) {
    let backoff = 1u64 << failed_attempt.min(6);
    let wait = retry_after_secs
        .map(|r| r.min(120))
        .unwrap_or(0)
        .max(backoff);
    tokio::time::sleep(Duration::from_secs(wait)).await;
}

/// First `max` characters, flattened to one line for status-line display.
fn truncate(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}...", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

/// Hide the API key before an URL shows up in errors or status output.
fn redact_url(url: &str) -> String {
    // only the account endpoint carries the key, as ?access_token=<key>
    match url.find("access_token=") {
        Some(i) => format!("{}access_token=<api-key>", &url[..i]),
        None => url.to_string(),
    }
}

fn url_to_cache_path(url: &str, cache_dir: &Path) -> PathBuf {
    let mut hash = DefaultHasher::new();
    url.hash(&mut hash);
    let hash = hash.finish();

    let mut path = cache_dir.to_owned();
    path.push(format!("{}{}", config::CACHE_PREFIX, hash));
    path
}
