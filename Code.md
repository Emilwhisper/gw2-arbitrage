# Code.md — Codebase Analysis

Reference for developers (and AI assistants) working on the GUI version of `gw2-arbitrage`.

## Overview

`gw2-arbitrage` is a Rust CLI tool that finds Guild Wars 2 items which can be crafted for less than their Trading Post (TP) sale revenue. It:

1. Downloads (or reads from cache) all items and recipes from the GW2 API.
2. Optionally merges custom/community recipes fetched from gw2efficiency.com.
3. Uses aggregated TP prices to cheaply estimate which recipes are profitable.
4. Uses full TP order book listings to compute exact profits (accounting for market liquidity), in parallel via rayon.
5. Prints a table of profitable items, optionally exports to CSV, and can print a detailed shopping list for a single item id (`gw2-arbitrage.exe <item-id>`).

Entry point: `src/main.rs` (`#[tokio::main] async fn main`). Library root: `src/lib.rs` — all modules are `pub`, so the profit pipeline is already usable as a library by a GUI.

Key crates: `tokio` (async), `reqwest` (HTTP), `rayon` (parallel profit calc), `structopt` (CLI), `serde`/`serde_json`, `bincode`+`flate2` (compressed cache files), `num-rational` (exact money arithmetic), `once_cell`/`lazy_static` (global config), `csv`, `colored`.


## Module-by-module analysis

### src/main.rs
- Loads account recipe unlocks if an API key is configured (`request::fetch_account_recipes`).
- Loads recipes (`request::get_data` on `CONFIG.api_recipes_file` → `request::request_paginated("recipes")`), discards recipes with empty disciplines.
- Loads custom recipes from gw2efficiency (`CONFIG.custom_recipes_file`), tolerating failure.
- Loads items (`CONFIG.items_file` → `request_paginated("items", lang)`), mapped `ApiItem → Item`.
- Merges custom recipes first, then API recipes (later insert wins on duplicate output item id), filters by recipe/item blacklists.
- Phase 1: fetch `/v2/commerce/prices` for all item ids, `profit::find_profitable_items` → cheap estimate of profitable item ids + ingredient ids.
- Phase 2: fetch `/v2/commerce/listings` for those ids, `profit::profitable_item_list` → exact `ProfitableItem` list (rayon).
- Optional disciplines filter (`--disciplines`), CSV writer (`--output-csv`, `OutputRow` struct), colored console table with columns: name, disciplines, item id, unknown recipes, total profit, number required, profit/item, crafting steps, profit/step, profit on cost.
- `item-id` arg: calls `profit::calc_item_profit` / shopping-list printing (detailed ingredient purchase plan, considers liquidity and vendor purchases).

### src/config.rs
- `Opt` (structopt): flags `--include-timegated`, `--include-ascended`, `--reset-cache`, `--config-file`, `--lang`, currency values (`--karma`, `--um`, `--vm`, `--ascended-value`, etc.), `--cache-dir`, `--disciplines`, `--count`, `--output-csv`, positional `item-id`.
- TOML config file at OS config dir `gw2-arbitrage.toml`: `api_key`, `lang`, `[currencies]` conversion values, `[blacklists]` item/recipe id lists.
- Global `CONFIG: Config` via `lazy_static!` — **note: the whole program reads this global at runtime; a GUI should populate it early or refactor toward passed-in options.**
- `Config` fields: `crafting: CraftingOptions { include_timegated, count, threshold, value }`, `output_csv`, `filter_disciplines`, `lang`, `api_key`, currency values (ascended/karma/um/vm/rn), cache/data file paths, blacklists, `item_id`.
- Cache dir defaults to OS cache dir (`dirs::cache_dir()`, on Windows `%LOCALAPPDATA%`) + `gw2-arbitrage`; data dir likewise; config file resolution in `config_file()`.
- `Discipline` enum (Armorsmith, Artificer, Chef, Huntsman, Jeweler, Leatherworker, Scribe, Tailor, Weaponsmith, Achievement) with abbreviations; `Language` enum (en/es/de/fr/zh/ru).
- `flush_cache` deletes `cache_*` files older than 5 minutes (API caches results 5 min).

### src/request.rs
- Constants: 10 parallel requests, page size 200, 200 item ids per batch.

### src/api.rs — GW2 API types
- `Price { id, buys: PriceInfo, sells: PriceInfo }`, `PriceInfo { unit_price, quantity }` — `/v2/commerce/prices`.
- `Recipe { id, output_item_id, output_item_count, time_to_craft_ms, disciplines, min_rating, flags: Vec<RecipeFlags>, ingredients }` — `/v2/recipes`. `RecipeFlags::AutoLearned | LearnedFromItem`; `is_purchased()`, `is_automatic()`.
- `ApiItem { id, name, item_type, rarity, level, vendor_value, flags, restrictions, upgrades_into, upgrades_from, details: Option<serde_json::Value> }` — `/v2/items`. Consumable details parsed into `item::Details`.
- `ItemListings { id, buys: Vec<Listing>, sells: Vec<Listing> }`, `Listing { listings, unit_price, quantity }` — `/v2/commerce/listings`.

### src/item.rs
- `Item` struct mirroring `ApiItem` (discards `details` except consumables, which become `Details::Consumable` with recipe unlock ids).
- `Type` enum (Armor, Weapon, Consumable, CraftingMaterial, Trinket, …), `Rarity` (Junk→Legendary) with localized names per language, `Flag` (AccountBound, NoSell, NoMysticForge, Unique, …).
- Important methods: `is_restricted()` (account/soulbound ⇒ can't sell on TP), `vendor_cost()` (vendor purchase price incl. currency-token special cases), `token_value()` (karma/UM/VM/RN item values when conversion rates configured), `recipe_unlocks()` (consumable → recipe ids).
- `Display` adds rarity in parentheses for trinkets (same name at multiple rarities).

### src/recipe.rs
- Unified `Recipe { id: Option<u32>, output_item_id, output_item_count, disciplines, ingredients, source }`.
- `RecipeSource`: Automatic | Discoverable | Purchasable | Achievement (derived from API flags / gw2efficiency data).
- `From<api::Recipe>` and `TryFrom<gw2efficiency::Recipe>` conversions; `is_timegated()` (based on `time_to_craft_ms`, gated by `--include-timegated`); `collect_ingredient_ids` recursive ingredient collection.

### src/crafting.rs
- `Source`: Crafting | TradingPost | Vendor. `EstimatedCraftingCost { cost, source }`.
- `calculate_estimated_min_crafting_cost(item_id, ...)` — recursive cheapest way to obtain an item: min of TP sell price, vendor/token cost, and crafting cost (recursed over ingredients ÷ output count). Skips timegated recipes unless enabled. Uses only aggregated prices — cheap, used for the first-pass profitability filter.

### src/profit.rs
- `find_profitable_items(tp_prices_map, recipes_map, items_map) → (Vec<item id>, Vec<ingredient id>)` — filters restricted items, discipline filter, requires sell-listing quantity > 0, compares effective sale revenue (minus TP fees) to estimated crafting cost.
- `profitable_item_list(...)` — rayon parallel exact profit via `calculate_crafting_profit` using full listings (liquidity-aware, computes optimal count).
- `calc_item_profit(item_id, ...)` — async path used for the single-item shopping list.
- `ProfitableItem` carries: `id`, `profit: Money`, `count`, `crafted_items`, `crafting_steps`; derived metrics `profit_per_item()`, `profit_per_crafting_step()`, `profit_on_cost()`, plus unknown-recipe detection vs `known_recipes` (account unlock set).

### src/money.rs
- `Money` = tuple of `Rational32` for copper, karma, um (unidentified magnets), vm (volatile magic), rn (research notes). Exact rational arithmetic; can't be reduced to a single number unless conversion rates are configured.
- TP fees: 5% listing fee + 10% exchange fee → `trading_post_sale_revenue()` = 85% of sale price.
- Display renders like `12g 34s 56c` (and non-coin currencies).


## GW2 API capabilities relevant to the GUI

Base URL: `https://api.guildwars2.com/v2` — no auth needed for public endpoints; API key needed for account endpoints.

| Endpoint | Used? | Notes |
|---|---|---|
| `/v2/items?page=&page_size=` | ✅ | All items. **Contains `icon`: full HTTPS URL to a 64×64 PNG icon — freely downloadable, ideal for the GUI.** Currently the `ApiItem → Item` conversion drops this field; it must be retained for the GUI. Other display fields: `name`, `rarity`, `type`, `level`, `vendor_value`, `flags`. |
| `/v2/recipes?page=&page_size=` | ✅ | All recipes. |
| `/v2/commerce/prices?ids=` | ✅ | Aggregated best buy/sell price + quantity. No auth. |
| `/v2/commerce/listings?ids=` | ✅ | Full order book (max 200 ids per call) — enables liquidity-aware profit. |
| `/v2/account/recipes?access_token=` | ✅ | Recipes unlocked by the account (used to flag "you don't know this recipe"). |
| `/v2/account/wallet?access_token=` | ❌ not yet | Could auto-fill karma/UM/VM balances for currency conversion in the GUI. |
| `/v2/files` | ❌ not needed | Static icon file list; item icons already come from `/v2/items`. |

Notes:
- API responses are cached by ArenaNet for 5 minutes; the tool mirrors this with its own file cache.
- Paging supports `page_size` up to 200.
- All endpoints relevant to this tool are free/rate-limit friendly; icons are plain CDN PNGs (no key, no CORS concerns for a desktop GUI).

### Icon caching (planned)

The `icon` URLs from `/v2/items` point to immutable PNGs, so they can be cached permanently on disk — unlike the 5-minute API response cache:
- Location: `icons/` subfolder of the cache dir, one file per item: `...\gw2-arbitrage\icons\<item_id>.png` (item id as filename → trivial lookup for the GUI).
- Cache lifecycle: `config.rs::flush_cache` only deletes `cache_*`-prefixed files, so icons survive both the 5-minute flush and `--reset-cache`.
- Download strategy: **lazy/on-demand** (download only icons actually displayed — list rows + detail window — to avoid fetching 60k+ icons on first run), with an optional "prefetch all" button that reuses the parallel `stream::iter(...).buffered(PARALLEL_REQUESTS)` pattern from `request.rs`.
- Storage format: plain PNG files on disk, not the bincode cache (bincode is for serde JSON types; raw images are better as files).
- GUI rendering: keep an in-memory map (`item_id → decoded image`) so the immediate-mode GUI doesn't re-read PNG files every frame; fall back to a placeholder when an icon is missing/unavailable.
- CLI mode ignores icons entirely.

## GUI integration notes

- `lib.rs` already exports everything (`pub mod ...`), so a GUI binary can reuse the data loading, crafting cost, and profit functions directly.
- Coupling points to be aware of:
  - Global `CONFIG` (built from CLI args at process start) is read all over `crafting.rs`, `profit.rs`, `item.rs`, `money.rs`. A GUI should initialize it once (e.g. from GUI settings) or refactor these call sites to take options as parameters.
  - `main.rs` mixes data loading, business logic and console output — for GUI mode, split "load data + compute list" into library functions returning structured data (`Vec<ProfitableItem>` + items/recipes maps) and let the GUI render.
  - Data loading is async (tokio); the GUI should run it on a background task and stream progress (e.g. via a channel) to avoid freezing the UI on first run.
- Plan for keeping CLI mode: keep `main.rs` console output; add a flag (`--cli`, or default-to-CLI when args/item-id are given) so both modes share one binary, with the GUI as the default launch path in the same crate.

### src/gw2efficiency.rs
- Fetches community "custom recipes" (JSON from gw2efficiency's API) for recipes not in the official API (mostly crafting conversions).
- Hard blacklist (phf set) of probabilistic/mystic-forge recipes with unreliable outputs (Mystic Clover, snowflakes, minis, swim-speed infusions, …).
- Lenient parsing (`treat_error_as_none`) to skip malformed entries.

- `fetch_item_listings(item_ids, cache_dir, notify)` — `/v2/commerce/listings?ids=…`, reverses buys/sells lists (best offer popped from back).
- `get_data(data_path, getter)` — bincode+deflate persistent cache ("download once, reuse").
- `request_paginated` — parallel page fetch for `/v2/<path>` with optional lang query.
- `request_item_ids` — batches ids (≤200) into `?ids=` calls, optionally file-cached per URL (hash of URL → `cache_<hash>` file).
- `fetch_account_recipes(key, ...)` — `/v2/account/recipes?access_token=` (hides the key from notify output).
- `fetch` — reqwest GET, error text extracted from JSON body, `serde_path_to_error` for good parse errors.
