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
- `flush_cache` deletes `cache_*` files older than 5 minutes (API caches results 5 min). Only `cache_*` names are ever considered for deletion (a name that is not valid UTF-8 is skipped explicitly - it used to fall through to the delete path), directories are skipped, and the age comes from `created()` with a `modified()` fallback for filesystems without birth times.

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
- Account-bound Mystic Forge legs are valued in `token_value()`: Mystic Crystal (20799) counts as free like karma basics, while Mystic Binding Agent (39125) is priced at one Bottle of Elonian Wine (2504c) - the cheapest coin cost it replaces - so the `recipe.rs` material-promotion rows cannot overstate their profit.
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
- Display renders like `0.12.34g` (gold.silver.copper, plus non-coin currencies); a genuinely zero value prints as `0c` instead of an empty string (empty parentheses in the GUI crafting tree used to look like a bug).


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

## GUI implementation (in progress, branch `GUI-test`)

New/changed modules:
- `src/analysis.rs` — library orchestration:
  - `load_analysis(notify) -> Analysis { items_map, recipes_map, known_recipes }` (all the data loading that used to live in `main.rs`).
  - `run_list_analysis(analysis, notify) -> Vec<ProfitableItem>` (prices → estimate → listings → exact profit).
  - `run_item_analysis(analysis, item_id, notify, refresh) -> (Option<ProfitableItem>, purchased_ingredients, unknown_recipes, prices, order_book)` (shopping list + the crafted item's own TP order book for one item).
  - `reset_data_files()` — deletes the cached items/recipes data files (icons + favorites preserved).
- `src/icons.rs` — icon disk cache (`icons/<item_id>.png` under the cache dir), `get_icon(item_id, icon_url, notify) -> Option<PathBuf>`, atomic `.tmp`+rename writes, permanent (never expires).
- `src/favorites.rs` — favorites persistence: `load()` / `save(&HashSet<u32>)` as `favorites.json` in the cache dir.
- `src/config.rs` — new `--cli` flag stored in `CONFIG.cli`; `CONFIG.icons_dir` added; **live runtime settings** as atomics so the GUI Settings window applies without a restart: `INCLUDE_TIMEGATED`, `INCLUDE_ASCENDED` (+ `ASCENDED_VALUE`) and `COUNT_LIMIT` (i64, `-1` = unlimited). Each is initialised from the CLI flag OR the matching TOML key (`include_timegated`, `include_ascended`, `count`) and read at calculation time by `crafting.rs` / `profit.rs`. GUI Settings writes `api_key`, `include_timegated`, `include_ascended` and `count` back to the TOML config file via `write_config_key`.
- `src/main.rs` — mode switch: **no arguments → GUI** (`gui::run()`); `--cli` or any arguments → console mode (`run_cli()`), unchanged behavior.
  - **Console window:** the crate sets `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`, so release builds are GUI-subsystem and double-clicking the exe opens no console window. Console mode then re-attaches to the invoking terminal via `win_console::attach_or_alloc()` (`AttachConsole(ATTACH_PARENT_PROCESS)`, falling back to `AllocConsole`) using hand-rolled `kernel32` FFI (no extra dependency). Two ordering constraints matter: it runs at the very top of `main`, (1) before any stdout write, because Rust captures the standard handles lazily on first use, and (2) before `CONFIG` is initialised, because `--help`/`--version` are printed by the argument parser during that initialisation. Debug builds keep the console subsystem so `cargo run` and tests behave normally. `main` also seeds `config::init_argv(std::env::args_os())` before `CONFIG` is touched, so the lazy global never parses a foreign `argv` (that is what makes `cargo test -- --nocapture`, `--test-threads=1` and test-name filters work; processes that never seed it, such as test binaries, get the CLI defaults). Note: the shell does not wait for a GUI-subsystem process, so CLI output appears just after the prompt returns.
- `src/gui.rs` — egui/eframe 0.27 app:
  - Toolbar: "Run analysis", "Reset cache & re-run", "Export CSV…" (rfd save dialog, same columns as CLI), "Prefetch icons" (caches icons for the listed items + favorites on 4 worker threads, progress in the toolbar; `Event::IconCached` just counts — no textures are created), "Settings", status/spinner.
  - List: sortable by clicking any column header (click again to flip direction; favorites stay pinned on top), per-row icon (lazy download via `icons.rs`, decoded to egui textures, placeholder on failure), ★ favorite toggle (pinned to top), favorites-only filter, discipline multi-filter checkboxes, and per-window velocity columns (only the enabled ones are rendered — the table is built dynamically from the enabled set). The list also has a "Filters" window: min velocity for one selected window, min profit % on cost, and min profit per crafted unit in copper - deliberately per unit, not the batch total, so a large `count` cannot pass a threshold that its unit margin fails. Drafts are persisted only when Apply is clicked.
  - Detail window on row click: icon, name, favorite toggle, links (GW2 wiki / gw2efficiency / gw2bltc), sell velocity per enabled window, profit summary (count, sell-at range, money required, breakeven), unknown-recipe warning, collapsible TP order book (top 5 asks/bids, sorted by best price), shopping-list grid (source: Crafting/TradingPost/Vendor, ingredient, count, min price, total cost).
  - Threading: analysis and item analysis run on background threads with their own tokio runtimes, communicating via `mpsc` events (`Progress`, `AnalysisDone`, `ItemDone`, `IconLoaded`, …); UI repaints while work is pending. Per-icon fetches (`request_icon`, one thread per un-cached visible row) also use a current-thread runtime: a multi-thread runtime would spawn a CPU-count-sized thread pool for every icon.
  - Worker pool sizing: velocity and icon prefetch spawn `App::worker_threads` threads (bound by `MIN_WORKER_THREADS` = 1, `DEFAULT_WORKER_THREADS` = 4, `MAX_WORKER_THREADS` = 16) reading one shared `Arc<Mutex<VecDeque<Job>>>`; the value is set from the Settings slider, persisted as `worker_threads` in `gui_prefs.json`, and clamped on load. Each worker builds a **current-thread** tokio runtime (`Builder::new_current_thread().enable_all()`) — it only ever blocks on one request at a time, so a multi-thread runtime would needlessly spawn a CPU-count-sized thread pool per worker. The number is read when workers are spawned, so changes apply to the next batch.
  - Settings window: API key, include-timegated, include-ascended, `--count` limit (all written to the TOML config and applied on the next analysis run), plus "Velocity windows" toggles and the "Background worker threads" slider (both persisted in `gui_prefs.json`).
- `src/velocity.rs` — sell-velocity estimates from the community datawars2.ie TP history API:
  - `fetch_velocity(item_id, fetch_hourly, fetch_daily) -> Velocity` with per-window units/day: 6h/12h/24h from the hourly endpoint, 7d/2w/1m/3m/6m/1y/2y from the daily endpoint (`start=YYYY-MM-DD` = today − 735 days; ISO — unix timestamps do NOT work, verified empirically; multi-ID is not supported either).
  - Only `sell_sold` counts (actual instant-buy sales); `sell_delisted` cancellations are ignored.
  - **HTTP client:** a single shared `static CLIENT: Lazy<reqwest::Client>` (30s timeout) is reused for every velocity request, so the connection pool / keep-alive survives across calls and TLS state is not rebuilt per request — this is what makes higher worker counts actually pay off. (Before, a fresh `reqwest::Client` was built inside `fetch_velocity` on every call.)
  - Coverage check: a window needs ≥80% of its expected buckets, else it reports `None` (shown as "–" in the GUI).
  - GUI Settings → "Velocity windows" toggles each window's column (6h…2y, all on by default); the selection is persisted under `velocity_windows` in `gui_prefs.json`. Disabling every hourly (or every daily) window skips that whole endpoint for each item — one request per item per endpoint — while extra windows inside an enabled group cost no additional requests.
  - Results are merged per endpoint group: `velocity_hourly_done` / `velocity_daily_done` record what was fetched per item, so enabling a window later (Settings) back-fills it without re-running the analysis and without discarding the other group's values.
  - **Disk cache:** `fetch_velocity_cached(cache_dir, item_id, fetch_hourly, fetch_daily)` wraps `fetch_velocity` and stores results as `velocity/<item_id>.json` in a `velocity/` subfolder of the cache dir (serde_json: `fetched_at`, `hourly`, `daily`, `velocity`) with a 24h TTL (`CACHE_TTL_SECS`). Fresh entries are reused, only missing groups are downloaded, and cached values are used as a fallback on network errors. The `velocity/` subfolder keeps the cache-dir root tidy and is not `cache_*`-prefixed, so it survives `flush_cache`; leftovers from the older `velocity_<item_id>.json` root layout are deleted once per process by `remove_legacy_files_once`.
  - The item detail window lists the velocity of every enabled window for that item, and fetches it on demand (`request_item_velocity`) when the item has not been processed by the workers yet.
- Cargo.toml additions: `eframe 0.27`, `egui_extras 0.27` (TableBuilder for the sortable, resizable, full-width table), `image 0.25` (png only), `rfd 0.12`.

Remaining known gaps (see `todo.md`): wallet auto-conversion; background auto-refresh; material-bank awareness; friendly error dialogs; app/window icon deliberately deferred (no icon is set in code today — the window is created with only `with_inner_size` + `with_title`, so adding one later is a one-line `ViewportBuilder::with_icon` change plus a `.ico`/PNG asset). A GUI-mode panic shows no dialog because the release build has no console — worth adding a panic hook/log file eventually.

- `lib.rs` already exports everything (`pub mod ...`), so a GUI binary can reuse the data loading, crafting cost, and profit functions directly.
- Coupling points to be aware of:
  - Global `CONFIG` (built from CLI args at process start) is read all over `crafting.rs`, `profit.rs`, `item.rs`, `money.rs`. A GUI should initialize it once (e.g. from GUI settings) or refactor these call sites to take options as parameters. Because `CONFIG` also merges the user's `gw2-arbitrage.toml` (which the GUI Settings window writes), `tests/profit.rs` calls `pin_live_options()` to force that file to load and then pin the live atomics - otherwise a developer's saved `count` / `include_*` settings change test outcomes (a saved `count = 1` used to fail `calculate_crafting_profit_with_output_item_count_test` locally while CI stayed green).
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
