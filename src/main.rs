//! Binary entry point.
//!
//! Launch behaviour:
//! - no arguments          -> graphical interface (no console window on Windows)
//! - `--cli` or any option -> the original console mode
//!
//! On Windows the *release* binary is built as a GUI-subsystem executable so
//! double-clicking it does not open a console window; console mode re-attaches
//! to the console of the invoking terminal (see `win_console`). Debug builds
//! keep the console so `cargo run` / tests behave normally.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use colored::Colorize;
use serde::Serialize;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::io::prelude::*;

use config::CONFIG;
use gw2_arbitrage::*;
use item::Item;
use money::Money;
use recipe::Recipe;

const ITEM_STACK_SIZE: u32 = 250; // GW2 uses a "stack size" of 250

/// Windows console handling for the GUI-subsystem release binary.
///
/// The release binary is built with `windows_subsystem = "windows"`, which means
/// it has no console of its own. Console mode therefore has to attach itself to
/// the console of the terminal that launched it, otherwise `--cli`, `--help` and
/// `--version` would print nothing at all.
#[cfg(all(windows, not(debug_assertions)))]
mod win_console {
    /// `ATTACH_PARENT_PROCESS` (a `DWORD` -1): attach to the parent's console.
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn AllocConsole() -> i32;
    }

    /// Attach to the invoking terminal's console so `println!` output is
    /// visible; if there is no parent console (e.g. launched from Explorer with
    /// arguments) fall back to creating a new one.
    ///
    /// Must run before anything writes to stdout/stderr: Rust captures the
    /// standard handles lazily on first use, so attaching first means the normal
    /// printing path picks up the console handles. It must also run before
    /// `CONFIG` is initialised, because `--help`/`--version` are printed by the
    /// argument parser during that initialisation.
    pub fn attach_or_alloc() {
        // SAFETY: plain Win32 console API calls with constant arguments. Failure
        // is reported through the return value and handled below.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any argument means console mode - including `--help`/`--version`, which the
    // argument parser prints - so re-attach to the invoking terminal's console
    // before anything writes to stdout. Only the bare double-click (no
    // arguments) opens the GUI without a console.
    #[cfg(all(windows, not(debug_assertions)))]
    if std::env::args().count() > 1 {
        win_console::attach_or_alloc();
    }

    // GUI mode when launched with no arguments (double-click on Windows);
    // CLI mode with `--cli` or any other arguments.
    if std::env::args().count() == 1 && !CONFIG.cli {
        gui::run().map_err(|e| e.to_string())?;
        return Ok(());
    }

    run_cli().await
}

async fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let notify_print = |url: &str| println!("Fetching {}", url);
    let notify = Some(&notify_print as &dyn Fn(&str));

    let analysis = analysis::load_analysis(notify).await?;
    println!(
        "Loaded {} recipes and {} items",
        analysis.recipes_map.len(),
        analysis.items_map.len()
    );

    if let Some(item_id) = CONFIG.item_id {
        let (
            profitable_item,
            purchased_ingredients,
            required_unknown_recipes,
            recipe_prices,
            _order_book,
        ) = analysis::run_item_analysis(&analysis, item_id, notify, false).await?;
        print_profitable_item(
            item_id,
            &profitable_item,
            &purchased_ingredients,
            &required_unknown_recipes,
            &recipe_prices,
            &analysis.recipes_map,
            &analysis.items_map,
            &analysis.known_recipes,
        )?;
    } else {
        println!("Loading trading post prices");
        print!("Pages:");
        let commerce_notify = |url: &str| {
            print!(" {}", &url[51..url.len() - 14]);
            io::stdout()
                .flush()
                .unwrap_or_else(|e| println!("Flush failed: {}", &e));
        };
        let profitable_items =
            analysis::run_list_analysis(&analysis, Some(&commerce_notify as &dyn Fn(&str))).await?;
        println!("");

        print_item_list(
            &profitable_items,
            &analysis.recipes_map,
            &analysis.items_map,
            &analysis.known_recipes,
        )?;
    }

    Ok(())
}

/// Print detailed information about a profitable item
fn print_profitable_item(
    item_id: u32,
    profitable_item: &Option<profit::ProfitableItem>,
    purchased_ingredients: &HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
    required_unknown_recipes: &Vec<u32>,
    recipe_prices: &HashMap<u32, api::Price>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    known_recipes: &Option<HashSet<u32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let profitable_item = if let Some(item) = profitable_item {
        item
    } else {
        println!("Item is not profitable to craft");
        return Ok(());
    };

    println!("============");
    println!(
        "Shopping list for {} x {} = {} profit ({} / step, {}%)",
        profitable_item.count,
        items_map
            .get(&item_id)
            .map_or_else(|| "???".to_string(), |item| item.to_string()),
        Money::from_copper(profitable_item.profit.to_copper_value()),
        profitable_item.profit_per_crafting_step().to_copper_value(),
        (profitable_item.profit_on_cost() * 100_f64).round(),
    );
    let price_msg = if profitable_item.max_sell == profitable_item.min_sell {
        format!("{}", profitable_item.min_sell)
    } else {
        format!(
            "{} to {}",
            profitable_item.max_sell, profitable_item.min_sell,
        )
    };
    println!(
        "Sell at: {}, Money Required: {}, Breakeven price: {}",
        price_msg,
        profitable_item.crafting_cost.increase_by_listing_fee(),
        profitable_item.breakeven,
    );

    println!("============");
    let mut sorted_ingredients: Vec<(&(u32, crafting::Source), &crafting::PurchasedIngredient)> =
        purchased_ingredients.iter().collect();
    sorted_ingredients.sort_unstable_by(|a, b| {
        if b.0 .1 == a.0 .1 {
            match b.1.count.cmp(&a.1.count) {
                Ordering::Equal => match b.1.total_cost.cmp(&a.1.total_cost) {
                    Ordering::Equal => b.0 .0.cmp(&a.0 .0),
                    v => v,
                },
                v => v,
            }
        } else if b.0 .1 == crafting::Source::Vendor {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    });
    let mut inventory = 0;
    for ((ingredient_id, ingredient_source), ingredient) in sorted_ingredients {
        let purchase_count = if *ingredient_source == crafting::Source::Vendor {
            items_map
                .get(ingredient_id)
                .unwrap_or_else(|| panic!("Missing item for ingredient {}", ingredient_id))
                .vendor_cost()
                .unwrap_or((Money::from_copper(0), 1))
                .1
        } else {
            ITEM_STACK_SIZE
        };
        let ingredient_count_msg = if purchase_count > 1 && ingredient.count > purchase_count {
            let stack_count = ingredient.count / purchase_count;
            inventory += ingredient.count.div_ceil(ITEM_STACK_SIZE);
            let remainder = ingredient.count % purchase_count;
            let remainder_msg = if remainder != 0 {
                format!(" + {}", remainder)
            } else {
                "".to_string()
            };
            format!(
                "{} ({} x {}{})",
                ingredient.count, stack_count, purchase_count, remainder_msg
            )
        } else {
            inventory += 1;
            ingredient.count.to_string()
        };
        let source_msg = match *ingredient_source {
            crafting::Source::TradingPost => {
                if ingredient.max_price == ingredient.min_price {
                    format!(
                        " (at {}) Subtotal: {}",
                        ingredient.min_price, ingredient.total_cost,
                    )
                } else {
                    format!(
                        " (at {} to {}) Subtotal: {}",
                        ingredient.min_price, ingredient.max_price, ingredient.total_cost,
                    )
                }
            }
            crafting::Source::Vendor => {
                let vendor_cost = items_map
                    .get(ingredient_id)
                    .unwrap_or_else(|| panic!("Missing item for ingredient {}", ingredient_id))
                    .vendor_cost();
                if let Some((cost, purchase_count)) = vendor_cost {
                    if purchase_count > 1 {
                        format!(
                            " (vendor: {} per {}) Subtotal: {}",
                            cost * purchase_count,
                            purchase_count,
                            cost * ingredient.count,
                        )
                    } else {
                        format!(" (vendor: {}) Subtotal: {}", cost, cost * ingredient.count,)
                    }
                } else {
                    "".to_string()
                }
            }
            crafting::Source::Crafting => "".to_string(),
        };
        println!(
            "{} {}{}",
            ingredient_count_msg,
            items_map
                .get(ingredient_id)
                .map_or_else(|| "???".to_string(), |item| item.to_string()),
            source_msg,
        );
    }

    println!("============");
    println!("Max inventory slots: {}", inventory + 1); // + 1 for the crafting output
    println!(
        "Crafting steps: https://gw2efficiency.com/crafting/calculator/a~1!b~1!c~1!d~{}-{}",
        profitable_item.count, item_id
    );
    for (item_id, count, recipe) in profitable_item.crafted_items.sorted(item_id, &recipes_map) {
        let num_crafted = count / recipe.output_item_count;
        let item_name = items_map
            .get(&item_id)
            .map_or_else(|| "???".to_string(), |item| item.to_string());
        let ingredients = recipe
            .sorted_ingredients()
            .iter()
            .map(|ingredient| {
                let ingredient_name = items_map
                    .get(&ingredient.item_id)
                    .map_or_else(|| "???".to_string(), |item| item.to_string());
                format!("{} {}", ingredient.count * num_crafted, ingredient_name)
            })
            .collect::<Vec<String>>()
            .join(" ");
        if recipe.output_item_count > 1 {
            println!(
                "{} (makes {}) {} from {}",
                num_crafted, count, item_name, ingredients
            );
        } else {
            println!("{} {} from {}", count, item_name, ingredients);
        }
    }

    if required_unknown_recipes.len() > 0 {
        let req_recipes = required_unknown_recipes
            .iter()
            .map(|id| {
                let recipe_names = items_map
                    .iter()
                    .filter(|(_, item)| {
                        if let Some(unlocks) = &item.recipe_unlocks() {
                            unlocks.iter().filter(|&recipe_id| id == recipe_id).count() > 0
                        } else {
                            false
                        }
                    })
                    .map(|(_, item)| {
                        // Need to get price, which means up at collect_ingredient_ids we'd need to
                        // also search for unknown recipes at all levels, and add those to the
                        // market list
                        if let Some(listing) = recipe_prices.get(&item.id) {
                            debug_assert!(listing.sells.unit_price < i32::MAX as u32);
                            return format!(
                                "{}, buy for {}",
                                &item.name,
                                Money::from_copper(listing.sells.unit_price as i32)
                            );
                        }
                        format!("{}", &item.name)
                    })
                    .collect::<Vec<String>>()
                    .join(" or ");
                if recipe_names.len() > 0 {
                    recipe_names
                } else {
                    // recipe 5424 for item 29407 has no unlock item, possibly others
                    format!("Recipe {} is not available!", &id)
                }
            })
            .collect::<Vec<String>>()
            .join("\n");
        println!(
            "You {} craft this yet. Required recipes{}:\n{}",
            match known_recipes {
                Some(_) => "can not",
                None => "may not be able to",
            },
            if required_unknown_recipes.len() > 1 {
                "s"
            } else {
                ""
            },
            req_recipes,
        );
    }

    if !profitable_item.crafted_items.leftovers.is_empty() {
        println!("Leftovers:");
        for (leftover_id, (count, cost, _)) in profitable_item.crafted_items.leftovers.iter() {
            println!(
                "{} {}, breakeven: {} each",
                count,
                items_map
                    .get(&leftover_id)
                    .map_or_else(|| "???".to_string(), |item| item.to_string()),
                cost.trading_post_listing_price(),
            );
        }
    }

    return Ok(());
}

#[derive(Debug, Serialize)]
struct OutputRow {
    name: String,
    disciplines: String,
    item_id: u32,
    unknown_recipes: Vec<u32>,
    total_profit: String,
    number_required: u32,
    profit_per_item: i32,
    crafting_steps: u32,
    profit_per_step: i32,
    profit_on_cost: f64,
}

/// List profitable items to screen or CSV
fn print_item_list(
    profitable_items: &Vec<profit::ProfitableItem>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    known_recipes: &Option<HashSet<u32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut csv_writer = if let Some(path) = &CONFIG.output_csv {
        Some(csv::Writer::from_path(path)?)
    } else {
        None
    };

    let mut line_colors = [
        colored::Color::Red,
        colored::Color::Green,
        colored::Color::Yellow,
        colored::Color::Magenta,
        colored::Color::Cyan,
    ]
    .iter()
    .cycle();

    let header = format!(
        "{:<50} {:<15} {:<15} {:<20} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
        "Name",
        "Disciplines",
        "Item id",
        "Req. Recipe Ids",
        "Total profit",
        "No. required",
        "Profit / item",
        "Crafting steps",
        "Profit / step",
        "Profit on cost",
    );

    println!("{}", header);
    println!("{}", "=".repeat(header.len()));
    for profitable_item in profitable_items {
        // Only required when prices are cached.
        // Profit may end up being 0, since potential profitable items are selected based
        // on cached prices, but the actual profit is calculated using detailed listings and
        // prices may have changed since they were cached.
        if profitable_item.count == 0 {
            continue;
        }

        let item_id = profitable_item.id;
        let name = items_map
            .get(&item_id)
            .map_or_else(|| "???".to_string(), |item| item.to_string());

        let recipe = recipes_map.get(&item_id).expect("Missing recipe");

        let output_row = OutputRow {
            name: name.to_string(),
            disciplines: recipe
                .disciplines
                .iter()
                .map(|d| d.get_abbrev())
                .collect::<Vec<_>>()
                .join("/"),
            item_id,
            unknown_recipes: profitable_item
                .crafted_items
                .unknown_recipes(&recipes_map, &known_recipes)
                .iter()
                .map(|&id| id)
                .collect(),
            total_profit: profitable_item.profit.to_string(),
            number_required: profitable_item.count,
            profit_per_item: profitable_item.profit_per_item().to_copper_value(),
            crafting_steps: profitable_item.crafting_steps,
            profit_per_step: profitable_item.profit_per_crafting_step().to_copper_value(),
            profit_on_cost: profitable_item.profit_on_cost(),
        };

        if let Some(writer) = &mut csv_writer {
            writer.serialize(&output_row)?;
        }

        let line = format!(
            "{:<50} {:<15} {:<15} {:<20} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
            output_row.name,
            output_row.disciplines,
            format!("{}", output_row.item_id),
            format!(
                "{}",
                output_row
                    .unknown_recipes
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
            ),
            output_row.total_profit,
            format!(
                "{} item{}",
                output_row.number_required,
                if output_row.number_required > 1 {
                    "s"
                } else {
                    ""
                }
            ),
            format!("{} / item", output_row.profit_per_item),
            format!("{} steps", output_row.crafting_steps),
            format!("{} / step", output_row.profit_per_step),
            format!("{}%", (output_row.profit_on_cost * 100_f64).round())
        );

        println!("{}", line.color(*line_colors.next().unwrap()));
    }

    println!("{}", "=".repeat(header.len()));
    println!("{}", header);
    println!("{}", "=".repeat(header.len()));

    let total_profit: Money = profitable_items.iter().map(|item| item.profit).sum();
    println!("Total: {}", total_profit);

    if let Some(writer) = &mut csv_writer {
        writer.flush()?;
    }

    Ok(())
}
