//! Library-level orchestration of the analysis pipeline.
//!
//! Loads the items/recipes databases (with caching) and computes profitable
//! items, returning structured data instead of printing. Both the CLI
//! (`main.rs`) and the future GUI call into these functions.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::api;
use crate::config::{self, CONFIG};
use crate::gw2efficiency;
use crate::item::Item;
use crate::profit;
use crate::recipe::{self, Recipe};
use crate::request;

/// Everything the analysis needs: item/recipe databases plus the account's
/// unlocked recipes (if an API key is configured).
pub struct Analysis {
    pub items_map: HashMap<u32, Item>,
    pub recipes_map: HashMap<u32, Recipe>,
    pub known_recipes: Option<HashSet<u32>>,
}

/// Load the cached-or-downloaded item and recipe databases, apply blacklists,
/// and build lookup maps. `notify` receives status messages (URLs) suitable
/// for progress display.
pub async fn load_analysis(notify: Option<&dyn Fn(&str)>) -> Result<Analysis, Box<dyn Error>> {
    let known_recipes = if let Some(key) = &CONFIG.api_key {
        match request::fetch_account_recipes(key, &CONFIG.cache_dir, notify).await {
            Ok(recipes) => Some(recipes),
            Err(error) => {
                eprintln!("API error fetching recipe unlocks: {}", error);
                None
            }
        }
    } else {
        None
    };

    let api_recipes = {
        let mut api_recipes: Vec<api::Recipe> = request::get_data(&CONFIG.api_recipes_file, || {
            request::request_paginated("recipes", &None, notify)
        })
        .await?;
        // If a recipe has no disciplines it cannot be crafted or discovered.
        // This appears to be used to mark deprecated recipes in the API.
        api_recipes.retain(|recipe| recipe.disciplines.len() > 0);
        api_recipes
    };

    let custom_recipes: Vec<Recipe> = request::get_data(&CONFIG.custom_recipes_file, || {
        gw2efficiency::fetch_custom_recipes(notify)
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("Failed to fetch custom recipes: {}", e);
        vec![]
    });

    let items: Vec<Item> = request::get_data(&CONFIG.items_file, || async {
        let api_items: Vec<api::ApiItem> =
            request::request_paginated("items", &CONFIG.lang, notify).await?;
        Ok(api_items
            .into_iter()
            .map(|api_item| Item::from(api_item))
            .collect())
    })
    .await?;

    let mut recipes: Vec<Recipe> = custom_recipes
        .into_iter()
        // prefer api recipes over custom recipes if they share the same output item id, by inserting them later
        .chain(api_recipes.into_iter().map(std::convert::From::from))
        .filter(|recipe| {
            if let Some(recipe_blacklist) = &CONFIG.recipe_blacklist {
                if let Some(id) = recipe.id {
                    if recipe_blacklist.contains(&id) {
                        return false;
                    }
                }
            }
            if let Some(item_blacklist) = &CONFIG.item_blacklist {
                for ingredient in &recipe.ingredients {
                    if item_blacklist.contains(&ingredient.item_id) {
                        return false;
                    }
                }
            }

            true
        })
        .collect();
    recipes.append(&mut Recipe::additional_recipes());
    let mut recipes_map = profit::vec_to_map(recipes, |x| x.output_item_id);
    let items_map = profit::vec_to_map(items, |x| x.id);

    let recursive_recipes = recipe::mark_recursive_recipes(&recipes_map);
    for recipe_id in recursive_recipes.into_iter() {
        recipes_map.remove(&recipe_id);
    }

    Ok(Analysis {
        items_map,
        recipes_map,
        known_recipes,
    })
}

/// How long a fetched market snapshot is reused for (matches the API's own
/// ~5-minute cache age).
pub const MARKET_SNAPSHOT_TTL_SECS: u64 = 300;

/// One download of everything the profit computation needs: aggregated prices
/// plus the detailed listings for all candidate items and their ingredients.
pub struct MarketSnapshot {
    pub prices: HashMap<u32, api::Price>,
    pub listings: HashMap<u32, api::ItemListings>,
    /// threshold the snapshot was fetched with: it covers any run whose
    /// threshold is greater or equal (a lower threshold only ever adds ids)
    pub threshold: i64,
    pub fetched_at: Instant,
}

impl MarketSnapshot {
    /// Fresh enough and fetched with a low-enough threshold to serve a run.
    pub fn covers(&self, threshold: i64) -> bool {
        self.threshold <= threshold
            && self.fetched_at.elapsed() < Duration::from_secs(MARKET_SNAPSHOT_TTL_SECS)
    }
}

/// Full list analysis: fetch aggregated TP prices, estimate profitable items,
/// then compute exact liquidity-aware profits using detailed listings.
/// Returns profitable items sorted by profit (ascending, as produced by
/// `profit::profitable_item_list`).
pub async fn run_list_analysis(
    analysis: &Analysis,
    notify: Option<&dyn Fn(&str)>,
) -> Result<Vec<profit::ProfitableItem>, Box<dyn Error>> {
    // CLI path: always fresh data
    let snapshot = fetch_market_snapshot(analysis, notify).await?;
    compute_profitable_items(analysis, &snapshot)
        .ok_or_else(|| "fresh market snapshot is missing listings".into())
}

/// Download a fresh market snapshot (prices + listings for all candidates).
pub async fn fetch_market_snapshot(
    analysis: &Analysis,
    notify: Option<&dyn Fn(&str)>,
) -> Result<MarketSnapshot, Box<dyn Error>> {
    let threshold = config::PROFIT_THRESHOLD.load(Ordering::Relaxed);
    let prices = fetch_relevant_prices(analysis, notify).await?;

    let (profitable_item_ids, ingredient_ids) =
        profit::find_profitable_items(&prices, &analysis.recipes_map, &analysis.items_map);

    let mut request_listing_item_ids = vec![];
    request_listing_item_ids.extend(&profitable_item_ids);
    request_listing_item_ids.extend(ingredient_ids);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();
    // Caching these is pointless, as the vector changes on each run, leading to new URLs
    let tp_listings = request::fetch_item_listings(&request_listing_item_ids, None, notify).await?;
    let listings = profit::vec_to_map(tp_listings, |x| x.id);

    Ok(MarketSnapshot {
        prices,
        listings,
        threshold,
        fetched_at: Instant::now(),
    })
}

/// Recompute the profitable-items list from a snapshot without any network
/// traffic. Returns `None` when the snapshot lacks listings the current
/// threshold requires (caller should fetch fresh instead).
pub fn compute_profitable_items(
    analysis: &Analysis,
    snapshot: &MarketSnapshot,
) -> Option<Vec<profit::ProfitableItem>> {
    let (profitable_item_ids, ingredient_ids) =
        profit::find_profitable_items(&snapshot.prices, &analysis.recipes_map, &analysis.items_map);

    let mut request_listing_item_ids = vec![];
    request_listing_item_ids.extend(&profitable_item_ids);
    request_listing_item_ids.extend(ingredient_ids);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();
    // Only the crafted items themselves must have order books (phase 2 reads
    // those unconditionally). Ingredient books may legitimately be absent —
    // untradable ingredients never have any, and vendor/crafted ones are never
    // read — mirroring how `profitable_item_list` skips missing entries.
    if !profitable_item_ids
        .iter()
        .all(|id| snapshot.listings.contains_key(id))
    {
        return None;
    }

    Some(profit::profitable_item_list(
        &snapshot.listings,
        &profitable_item_ids,
        &request_listing_item_ids,
        &analysis.recipes_map,
        &analysis.items_map,
    ))
}

/// Above this many price-relevant ids, a full paginated dump is cheaper than
/// one `?ids=` batch per 200 ids (~140 dump pages vs batches of 200).
const TARGETED_PRICES_MAX_IDS: usize = 20_000;

/// All item ids whose TP price the profitability estimate can possibly need:
/// every recipe output plus every transitive ingredient.
fn relevant_price_ids(recipes_map: &HashMap<u32, Recipe>) -> Vec<u32> {
    let mut ids = Vec::with_capacity(recipes_map.len() * 2);
    for (output_id, recipe) in recipes_map {
        ids.push(*output_id);
        recipe.collect_ingredient_ids(recipes_map, &mut ids);
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Fetch aggregated TP prices, restricted to recipe-relevant ids when that is
/// cheaper than the full dump. The estimate only ever looks up prices for
/// recipe outputs and (transitive) ingredients, so the result is equivalent.
async fn fetch_relevant_prices(
    analysis: &Analysis,
    notify: Option<&dyn Fn(&str)>,
) -> Result<HashMap<u32, api::Price>, Box<dyn Error>> {
    let ids = relevant_price_ids(&analysis.recipes_map);
    if ids.len() <= TARGETED_PRICES_MAX_IDS {
        let tp_prices: Vec<api::Price> =
            request::request_item_ids("commerce/prices", &ids, None, notify, true).await?;
        Ok(profit::vec_to_map(tp_prices, |x| x.id))
    } else {
        let tp_prices: Vec<api::Price> =
            request::request_paginated("commerce/prices", &None, notify).await?;
        Ok(profit::vec_to_map(tp_prices, |x| x.id))
    }
}

/// Single-item analysis: shopping list data for one item id.
/// When `refresh` is true, bypasses the listings cache for fresh prices.
pub async fn run_item_analysis(
    analysis: &Analysis,
    item_id: u32,
    notify: Option<&dyn Fn(&str)>,
    refresh: bool,
) -> Result<
    (
        Option<profit::ProfitableItem>,
        HashMap<(u32, crate::crafting::Source), crate::crafting::PurchasedIngredient>,
        Vec<u32>,
        HashMap<u32, api::Price>,
        Option<api::ItemListings>,
    ),
    Box<dyn Error>,
> {
    profit::calc_item_profit(
        item_id,
        &analysis.recipes_map,
        &analysis.items_map,
        &analysis.known_recipes,
        notify,
        refresh,
    )
    .await
}

/// Delete the cached items/recipes data files so the next run re-downloads
/// them from the GW2 API. Icons and favorites are preserved.
/// Returns the paths that were removed.
pub fn reset_data_files() -> Vec<std::path::PathBuf> {
    let mut removed = vec![];
    for file in [
        &CONFIG.items_file,
        &CONFIG.api_recipes_file,
        &CONFIG.custom_recipes_file,
    ] {
        if file.exists() {
            let _ = std::fs::remove_file(file);
            removed.push(file.clone());
        }
    }
    removed
}
