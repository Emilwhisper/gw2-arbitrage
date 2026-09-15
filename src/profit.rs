use rayon::prelude::*;

use num_traits::Zero;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::api;
use crate::config;
use crate::crafting;
use crate::item::Item;
use crate::money::Money;
use crate::recipe::Recipe;
use crate::request;
use config::CONFIG;

/// Return a items which are profitable to make at least one of, and their ingredients, for further
/// scrutiny
pub fn find_profitable_items(
    tp_prices_map: &HashMap<u32, api::Price>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
) -> (Vec<u32>, Vec<u32>) {
    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    // live threshold (GUI normal/wide modes); 0 keeps the historical
    // strictly-profitable behavior byte-identical
    let threshold = Money::from_copper(
        config::PROFIT_THRESHOLD.load(std::sync::atomic::Ordering::Relaxed) as i32,
    );
    let patient = config::PRICE_PATIENT.load(std::sync::atomic::Ordering::Relaxed);
    for (item_id, recipe) in recipes_map {
        if let Some(item) = items_map.get(item_id) {
            // we cannot sell restricted items
            if item.is_restricted() {
                continue;
            }
        }

        if let Some(filter_disciplines) = &CONFIG.filter_disciplines {
            let mut has_discipline = false;
            for discipline in filter_disciplines {
                if recipe.disciplines.iter().any(|s| s == discipline) {
                    has_discipline = true;
                    break;
                }
            }

            if !has_discipline {
                continue;
            }
        }

        // some items are craftable and have no listed restrictions but are still not listable on tp
        // e.g. 39417, 79557
        // conversely, some items have a NoSell flag but are listable on the trading post
        // e.g. 66917
        let tp_prices = match tp_prices_map.get(item_id) {
            Some(tp_prices) if tp_prices.sells.quantity > 0 => tp_prices,
            _ => continue,
        };

        if let Some(crafting::EstimatedCraftingCost {
            source: crafting::Source::Crafting,
            cost: crafting_cost,
        }) = crafting::calculate_estimated_min_crafting_cost(
            *item_id,
            &recipes_map,
            &items_map,
            &tp_prices_map,
            &CONFIG.crafting,
        ) {
            // effective sale revenue: top bid when instant, cheapest ask when
            // patient (both minus trading-post fees)
            let effective_buy_price = Money::from_copper(if patient {
                tp_prices.sells.unit_price as i32
            } else {
                tp_prices.buys.unit_price as i32
            })
            .trading_post_sale_revenue();
            if effective_buy_price > crafting_cost + threshold {
                profitable_item_ids.push(*item_id);
                if let Some(recipe) = recipes_map.get(&item_id) {
                    recipe.collect_ingredient_ids(&recipes_map, &mut ingredient_ids);
                }
            }
        }
    }

    (profitable_item_ids, ingredient_ids)
}

/// Compute exact profit of profitable items independently in parallel
pub fn profitable_item_list(
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    profitable_item_ids: &Vec<u32>,
    request_listing_item_ids: &Vec<u32>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
) -> Vec<ProfitableItem> {
    let mut profitable_items: Vec<_> = profitable_item_ids
        .par_iter()
        .filter_map(|item_id| {
            let mut ingredient_ids = vec![*item_id];
            if let Some(recipe) = recipes_map.get(&item_id) {
                recipe.collect_ingredient_ids(&recipes_map, &mut ingredient_ids);
            }

            let mut tp_listings_map_for_item: HashMap<u32, _> = HashMap::new();
            for id in ingredient_ids {
                debug_assert!(request_listing_item_ids.contains(&id));
                if let Some(listing) = tp_listings_map.get(&id).cloned() {
                    tp_listings_map_for_item.insert(id, listing);
                }
            }

            calculate_crafting_profit(
                *item_id,
                &recipes_map,
                &items_map,
                &tp_listings_map_for_item,
                None,
                &CONFIG.crafting,
            )
        })
        .collect();

    profitable_items.sort_unstable_by_key(|item| item.profit);

    profitable_items
}

pub async fn calc_item_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    known_recipes: &Option<HashSet<u32>>,
    notify: Option<&dyn Fn(&str)>,
    // when true, bypass the listings cache and fetch fresh prices
    refresh: bool,
) -> Result<
    (
        Option<ProfitableItem>,
        HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
        Vec<u32>,
        HashMap<u32, api::Price>,
        // the crafted item's own TP order book (bids + asks), for display
        Option<api::ItemListings>,
    ),
    Box<dyn std::error::Error>,
> {
    let mut items_to_price = vec![];

    let mut unknown_recipes = HashSet::new();
    let mut recipe_prices = Default::default();
    if let Some(recipe) = recipes_map.get(&item_id) {
        recipe.collect_ingredient_ids(&recipes_map, &mut items_to_price);

        recipe.collect_unknown_recipe_ids(&recipes_map, &known_recipes, &mut unknown_recipes);
        let recipe_items: Vec<u32> = items_map
            .iter()
            .filter_map(|(_, item)| {
                if let Some(unlocks) = &item.recipe_unlocks() {
                    if unlocks
                        .iter()
                        .filter(|&recipe_id| unknown_recipes.contains(recipe_id))
                        .count()
                        > 0
                    {
                        return Some(item.id);
                    }
                }
                None
            })
            .collect();
        let prices: Vec<api::Price> =
            request::request_item_ids("commerce/prices", &recipe_items, None, notify, false)
                .await
                .unwrap_or(Default::default()); // ignore "all ids provided are invalid" (and all other errors)
        recipe_prices = vec_to_map(prices, |x| x.id);
    }

    let mut request_listing_item_ids = vec![item_id];
    request_listing_item_ids.extend(items_to_price);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();

    let listings_cache_dir: Option<&std::path::PathBuf> = if refresh {
        None
    } else {
        Some(&CONFIG.cache_dir)
    };
    let tp_listings =
        request::fetch_item_listings(&request_listing_item_ids, listings_cache_dir, notify).await?;
    let tp_listings_map = vec_to_map(tp_listings, |x| x.id);

    let mut purchased_ingredients = Default::default();
    let profitable_item = calculate_crafting_profit(
        item_id,
        &recipes_map,
        &items_map,
        &tp_listings_map,
        Some(&mut purchased_ingredients),
        &CONFIG.crafting,
    );

    let required_unknown_recipes: Vec<u32> = if let Some(profitable_item) = &profitable_item {
        profitable_item
            .crafted_items
            .crafted
            .keys()
            .filter_map(|item_id| {
                if let Some(recipe) = recipes_map.get(&item_id) {
                    if let Some(recipe_id) = recipe.id {
                        if unknown_recipes.contains(&recipe_id) {
                            return Some(recipe_id);
                        }
                    }
                }
                None
            })
            .collect()
    } else {
        Default::default()
    };

    Ok((
        profitable_item,
        purchased_ingredients,
        required_unknown_recipes,
        recipe_prices,
        tp_listings_map.get(&item_id).cloned(),
    ))
}

pub fn calculate_crafting_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    mut purchased_ingredients: Option<
        &mut HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
    >,
    opt: &config::CraftingOptions,
) -> Option<ProfitableItem> {
    let mut tp_listings_map: BTreeMap<u32, ItemListings> = tp_listings_map
        .clone()
        .into_iter()
        .map(|(id, listings)| (id, ItemListings::from(listings)))
        .collect();

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);
    // live threshold shared with `find_profitable_items` (GUI normal/wide
    // modes); initialized from `--threshold`, so CLI behavior is unchanged
    let threshold = Money::from_copper(
        config::PROFIT_THRESHOLD.load(std::sync::atomic::Ordering::Relaxed) as i32,
    );
    // live price mode shared with `find_profitable_items` (GUI Instant /
    // Patient toggle); initialized from `--patient`, so CLI behavior is
    // unchanged (instant unless flagged)
    let patient = config::PRICE_PATIENT.load(std::sync::atomic::Ordering::Relaxed);

    let mut listing_profit = Money::zero();
    let mut total_crafting_cost = Money::zero();
    let mut crafting_count = 0;
    let mut crafted_items = crafting::CraftedItems::default();

    let mut min_sell = 0;
    // best price the product can fetch right now: top bid when instant,
    // cheapest ask when patient (both before fees)
    let max_sell = tp_listings_map.get(&item_id).map_or_else(
        || opt.threshold.unwrap_or(0),
        |listings| {
            if patient {
                listings.sells.last().map_or(0, |l| l.unit_price)
            } else {
                listings.buys.last().map_or(0, |l| l.unit_price)
            }
        },
    );
    let mut breakeven = Money::zero();

    // simulate crafting 1 item per loop iteration until it becomes unprofitable
    let count_limit = config::COUNT_LIMIT.load(std::sync::atomic::Ordering::Relaxed);
    let count_limit = if count_limit < 0 {
        None
    } else {
        Some(count_limit as u32)
    };
    loop {
        if let Some(count) = count_limit {
            if crafting_count + output_item_count > count {
                break;
            }
        }

        let mut context = crafting::PreciseCraftingCostContext {
            purchases: vec![],
            items: crafted_items.clone(),
        };

        let crafting_cost = if let Some(crafting::PreciseCraftingCost {
            source: crafting::Source::Crafting,
            cost,
        }) = crafting::calculate_precise_min_crafting_cost(
            item_id,
            output_item_count,
            recipes_map,
            items_map,
            &mut tp_listings_map,
            &mut context,
            opt,
        ) {
            cost
        } else {
            break;
        };

        let (buy_price, min_buy) = if let Some(price) = opt.value {
            (Money::from_copper(price as i32) * output_item_count, price)
        } else if let Some((buy_price, min_buy)) = tp_listings_map
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id))
            .sell_with_mode(output_item_count, patient)
        {
            (buy_price, min_buy)
        } else {
            break;
        };

        // Ensure buy_price is larger before subtracting cost for profit
        if buy_price < crafting_cost + threshold {
            break;
        }
        // In wide (negative-threshold) mode, stop before the running total
        // breaches the bound: otherwise a mildly profitable top of the book
        // gets dragged under -1g by its own deep order book, and the item
        // vanishes from the wide list even though the normal run shows it.
        // Never fires for threshold >= 0, so normal/CLI runs are unaffected.
        if threshold < Money::zero() && listing_profit + (buy_price - crafting_cost) < threshold {
            break;
        }

        listing_profit += buy_price - crafting_cost;
        total_crafting_cost += crafting_cost;
        crafting_count += output_item_count;
        crafted_items = context.items;

        min_sell = min_buy;
        // Breakeven is based on the last/most expensive to craft
        breakeven = crafting_cost / output_item_count;

        // Finalize purchases
        for (purchase_id, count, purchase_source) in &context.purchases {
            let (cost, min_sell, max_sell) = if let crafting::Source::TradingPost = *purchase_source
            {
                let listing = tp_listings_map.get_mut(purchase_id).unwrap_or_else(|| {
                    panic!(
                        "Missing listings for ingredient {} of item id {}",
                        purchase_id, item_id
                    )
                });
                if patient {
                    listing.pending_sell_quantity -= *count;
                } else {
                    listing.pending_buy_quantity -= *count;
                }
                let (cost, min_sell, max_sell) =
                    listing.buy_with_mode(*count, patient).unwrap_or_else(|| {
                        panic!(
                            "Expected to be able to buy {} of ingredient {} for item id {}",
                            count, purchase_id, item_id
                        )
                    });
                (cost, min_sell, max_sell)
            } else {
                (0, 0, 0)
            };

            if let Some(purchased_ingredients) = &mut purchased_ingredients {
                let ingredient = purchased_ingredients
                    .entry((*purchase_id, *purchase_source))
                    .or_insert_with(|| crafting::PurchasedIngredient {
                        count: 0,
                        max_price: Money::default(),
                        min_price: Money::default(),
                        total_cost: Money::default(),
                    });
                ingredient.count += count;
                // track the true range over all batches (asks ascend as the
                // book depletes, but don't rely on that: compare properly
                // instead of the old is-zero sentinel, which broke on
                // genuine zero prices)
                let batch_min = Money::from_copper(min_sell as i32);
                let batch_max = Money::from_copper(max_sell as i32);
                if ingredient.count == *count {
                    // first batch recorded for this entry
                    ingredient.min_price = batch_min;
                    ingredient.max_price = batch_max;
                } else {
                    ingredient.min_price = ingredient.min_price.min(batch_min);
                    ingredient.max_price = ingredient.max_price.max(batch_max);
                }
                ingredient.total_cost += Money::from_copper(cost as i32);
            }
        }
        debug_assert!(tp_listings_map.iter().all(|(_, listing)| {
            listing.pending_buy_quantity == 0 && listing.pending_sell_quantity == 0
        }));
    }

    if crafting_count > 0 && !listing_profit.is_zero() {
        Some(ProfitableItem {
            id: item_id,
            crafting_cost: total_crafting_cost,
            profit: listing_profit,
            count: crafting_count,
            max_sell: Money::from_copper(max_sell as i32),
            min_sell: Money::from_copper(min_sell as i32),
            breakeven: breakeven.trading_post_listing_price(),
            crafting_steps: crafted_items.crafting_steps(recipes_map).to_integer(),
            crafted_items,
        })
    } else {
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProfitableItem {
    pub id: u32,
    pub crafting_cost: Money,
    pub count: u32,
    pub profit: Money,
    pub max_sell: Money,
    pub min_sell: Money,
    pub breakeven: Money,
    pub crafting_steps: u32,
    pub crafted_items: crafting::CraftedItems,
}

impl ProfitableItem {
    pub fn profit_per_item(&self) -> Money {
        self.profit / self.count
    }

    pub fn profit_per_crafting_step(&self) -> Money {
        // guard against zero steps (unreachable today, but cheap insurance
        // for recalculation paths with forced sources)
        if self.crafting_steps == 0 {
            Money::zero()
        } else {
            self.profit / self.crafting_steps
        }
    }

    pub fn profit_on_cost(&self) -> f64 {
        self.profit.percent(self.crafting_cost)
    }
}

#[derive(Clone, Debug)]
pub struct ItemListings {
    pub id: u32,
    pub buys: Vec<Listing>,
    pub sells: Vec<Listing>,
    pub pending_buy_quantity: u32,
    pub pending_sell_quantity: u32,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub unit_price: u32,
    pub quantity: u32,
}

impl ItemListings {
    fn buy(&mut self, mut count: u32) -> Option<(u32, u32, u32)> {
        let mut cost = 0;
        let mut min_sell = 0;
        let mut max_sell = 0;

        while count > 0 {
            // sells are sorted in descending price
            let remove = if let Some(listing) = self.sells.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                if min_sell == 0 {
                    min_sell = listing.unit_price;
                }
                max_sell = listing.unit_price;
                cost += listing.unit_price;
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.sells.pop();
            }
        }

        Some((cost, min_sell, max_sell))
    }

    fn sell(&mut self, mut count: u32) -> Option<(Money, u32)> {
        let mut revenue = Money::zero();
        let mut min_buy = 0;

        while count > 0 {
            // buys are sorted in ascending price
            let remove = if let Some(listing) = self.buys.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                min_buy = listing.unit_price;
                revenue +=
                    Money::from_copper(listing.unit_price as i32).trading_post_sale_revenue();
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.buys.pop();
            }
        }

        Some((revenue, min_buy))
    }

    /// Patient counterpart of `buy`: place buy orders, filling from the best
    /// bids down. Returns (cost, highest bid paid, lowest bid paid).
    pub fn buy_at_bid(&mut self, mut count: u32) -> Option<(u32, u32, u32)> {
        let mut cost = 0;
        let mut min_buy = 0;
        let mut max_buy = 0;

        while count > 0 {
            // buys are sorted in ascending price
            let remove = if let Some(listing) = self.buys.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                if min_buy == 0 {
                    min_buy = listing.unit_price;
                }
                max_buy = listing.unit_price;
                cost += listing.unit_price;
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.buys.pop();
            }
        }

        Some((cost, min_buy, max_buy))
    }

    /// Patient counterpart of `sell`: list at sell orders, filling from the
    /// cheapest asks up. Returns (revenue after fees, highest ask filled).
    pub fn sell_at_ask(&mut self, mut count: u32) -> Option<(Money, u32)> {
        let mut revenue = Money::zero();
        let mut max_ask = 0;

        while count > 0 {
            // sells are sorted in descending price
            let remove = if let Some(listing) = self.sells.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                max_ask = listing.unit_price;
                revenue +=
                    Money::from_copper(listing.unit_price as i32).trading_post_sale_revenue();
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.sells.pop();
            }
        }

        Some((revenue, max_ask))
    }

    /// Revenue side dispatcher: instant sells into bids, patient lists at asks.
    fn sell_with_mode(&mut self, count: u32, patient: bool) -> Option<(Money, u32)> {
        if patient {
            self.sell_at_ask(count)
        } else {
            self.sell(count)
        }
    }

    /// Cost side dispatcher: instant buys at asks, patient places bids.
    fn buy_with_mode(&mut self, count: u32, patient: bool) -> Option<(u32, u32, u32)> {
        if patient {
            self.buy_at_bid(count)
        } else {
            self.buy(count)
        }
    }

    pub fn lowest_sell_offer(&self, mut quantity: u32) -> Option<u32> {
        debug_assert!(!quantity.is_zero());

        let mut cost = 0;
        let mut pending_buy_quantity = self.pending_buy_quantity;

        for listing in self.sells.iter().rev() {
            let mut remaining_listing_quantity = listing.quantity;
            if pending_buy_quantity > 0 {
                if pending_buy_quantity >= remaining_listing_quantity {
                    pending_buy_quantity -= remaining_listing_quantity;
                    remaining_listing_quantity = 0;
                } else {
                    remaining_listing_quantity -= pending_buy_quantity;
                    pending_buy_quantity = 0;
                }
            }

            if remaining_listing_quantity > 0 {
                if remaining_listing_quantity < quantity {
                    quantity -= remaining_listing_quantity;
                    cost += remaining_listing_quantity * listing.unit_price;
                } else {
                    cost += quantity * listing.unit_price;
                    quantity = 0;
                }
            }

            if quantity.is_zero() {
                break;
            }
        }

        if quantity > 0 {
            None
        } else {
            Some(cost)
        }
    }

    /// Patient counterpart of `lowest_sell_offer`: cheapest total for placing
    /// buy orders, walking down from the best bids and skipping quantities
    /// already reserved by pending patient purchases.
    pub fn highest_buy_offer(&self, mut quantity: u32) -> Option<u32> {
        debug_assert!(!quantity.is_zero());

        let mut cost = 0;
        let mut pending_sell_quantity = self.pending_sell_quantity;

        for listing in self.buys.iter().rev() {
            let mut remaining_listing_quantity = listing.quantity;
            if pending_sell_quantity > 0 {
                if pending_sell_quantity >= remaining_listing_quantity {
                    pending_sell_quantity -= remaining_listing_quantity;
                    remaining_listing_quantity = 0;
                } else {
                    remaining_listing_quantity -= pending_sell_quantity;
                    pending_sell_quantity = 0;
                }
            }

            if remaining_listing_quantity > 0 {
                if remaining_listing_quantity < quantity {
                    quantity -= remaining_listing_quantity;
                    cost += remaining_listing_quantity * listing.unit_price;
                } else {
                    cost += quantity * listing.unit_price;
                    quantity = 0;
                }
            }

            if quantity.is_zero() {
                break;
            }
        }

        if quantity > 0 {
            None
        } else {
            Some(cost)
        }
    }

    /// Cost-estimate dispatcher: cheapest ask when instant, best bid when
    /// patient (both skipping quantities reserved by pending purchases).
    pub fn best_offer_with_mode(&self, quantity: u32, patient: bool) -> Option<u32> {
        if patient {
            self.highest_buy_offer(quantity)
        } else {
            self.lowest_sell_offer(quantity)
        }
    }
}

impl From<api::ItemListings> for ItemListings {
    fn from(v: api::ItemListings) -> Self {
        ItemListings {
            id: v.id,
            buys: v
                .buys
                .into_iter()
                .map(|listing| Listing {
                    unit_price: listing.unit_price,
                    quantity: listing.quantity.into(),
                })
                .collect(),
            sells: v
                .sells
                .into_iter()
                .map(|listing| Listing {
                    unit_price: listing.unit_price,
                    quantity: listing.quantity.into(),
                })
                .collect(),
            pending_buy_quantity: 0,
            pending_sell_quantity: 0,
        }
    }
}

pub fn vec_to_map<T, F>(v: Vec<T>, id_fn: F) -> HashMap<u32, T>
where
    F: Fn(&T) -> u32,
{
    let mut map = HashMap::default();
    for x in v.into_iter() {
        map.insert(id_fn(&x), x);
    }
    map
}
