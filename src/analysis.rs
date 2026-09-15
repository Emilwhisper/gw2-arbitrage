//! Library-level orchestration of the analysis pipeline.
//!
//! Loads the items/recipes databases (with caching) and computes profitable
//! items, returning structured data instead of printing. Both the CLI
//! (`main.rs`) and the future GUI call into these functions.

use std::collections::{HashMap, HashSet};
use std::error::Error;

use crate::api;
use crate::config::CONFIG;
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
pub async fn load_analysis(
    notify: Option<&dyn Fn(&str)>,
) -> Result<Analysis, Box<dyn Error>> {
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

/// Full list analysis: fetch aggregated TP prices, estimate profitable items,
/// then compute exact liquidity-aware profits using detailed listings.
/// Returns profitable items sorted by profit (ascending, as produced by
/// `profit::profitable_item_list`).
pub async fn run_list_analysis(
    analysis: &Analysis,
    notify: Option<&dyn Fn(&str)>,
) -> Result<Vec<profit::ProfitableItem>, Box<dyn Error>> {
    let tp_prices: Vec<api::Price> =
        request::request_paginated("commerce/prices", &None, notify).await?;
    let tp_prices_map = profit::vec_to_map(tp_prices, |x| x.id);

    let (profitable_item_ids, ingredient_ids) = profit::find_profitable_items(
        &tp_prices_map,
        &analysis.recipes_map,
        &analysis.items_map,
    );

    let mut request_listing_item_ids = vec![];
    request_listing_item_ids.extend(&profitable_item_ids);
    request_listing_item_ids.extend(ingredient_ids);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();
    // Caching these is pointless, as the vector changes on each run, leading to new URLs
    let tp_listings =
        request::fetch_item_listings(&request_listing_item_ids, None, notify).await?;
    let tp_listings_map = profit::vec_to_map(tp_listings, |x| x.id);

    let profitable_items = profit::profitable_item_list(
        &tp_listings_map,
        &profitable_item_ids,
        &request_listing_item_ids,
        &analysis.recipes_map,
        &analysis.items_map,
    );

    Ok(profitable_items)
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

