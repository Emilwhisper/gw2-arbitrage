# todo.md — GUI Roadmap

Phased plan to add a Windows-first GUI to `gw2-arbitrage` while keeping the CLI mode fully functional.

## Phase 0 — Refactor for GUI readiness
- [x] Retain the `icon` URL in `Item` (from `/v2/items`) instead of dropping it in the `ApiItem → Item` conversion (`#[serde(default)]` so old cached item DBs still parse; may require a one-time items re-download / cache bump).
- [x] Extract data loading + profit pipeline from `main.rs` into library functions returning structured data (e.g. `Analysis { items, recipes, profitable_items }`) instead of printing directly.
- [x] Reduce reliance on the global `CONFIG` for the mutable crafting options: `INCLUDE_TIMEGATED`, `INCLUDE_ASCENDED` and `COUNT_LIMIT` are live atomics (CLI flag OR TOML key) read at calculation time, so GUI Settings changes apply to the next run. Remaining: the read-only paths (paths, languages, blacklists) are still only initialised once at startup, which is fine for a GUI launched from the same config.
- [x] Add `--cli` flag / argument detection so the single binary can run in either console or GUI mode. (No args or `--cli` absent logic: no args → GUI; `--cli` or any arguments → CLI.)
- [x] Icon caching: new `icons.rs` library module — `get_icon(item_id, url, notify) -> Option<PathBuf>` (check disk → download PNG → save → return path). Uses `CONFIG.icons_dir` (`<cache-dir>\icons`).
  - [x] Store raw PNGs in a permanent `icons/` subfolder of the cache dir (`...\gw2-arbitrage\icons\<item_id>.png`); icons are immutable, no expiry, and `flush_cache` only deletes `cache_*` files so they survive `--reset-cache`.
  - [x] GUI renders item icons from the `icons.rs` disk cache: lazy per-row download on a background thread, decoded to egui textures, placeholder on failure.
  - [x] In-memory map of decoded images (`icon_textures: HashMap<u32, TextureHandle>`) so egui doesn't re-read files per frame.
  - [x] "Prefetch icons" toolbar button: downloads/caches icons for every row in the list plus all favorites (skipping already-cached ones) with 4 worker threads and a progress indicator; icons are cached to disk, so this makes later runs instant/offline-friendly. (Only the already-displayed ids are prefetched — not the whole 60k item DB.)
  - [x] CLI mode ignores icons entirely (no impact on console output).
- [x] Add shopping-list rendering as a function returning data (not just `println!`), so the detail window can reuse it.

## Phase 1 — GUI framework and basic window
- [x] Choose framework. → **egui/eframe 0.27** (pure Rust, lightweight, great Windows support).
- [x] Basic window: "Run analysis" button with progress indicator (background tokio task + mpsc channel).
- [x] Show the profitable-items list in a sortable table: name, disciplines, item id, total profit, profit/item, profit/step.
- [x] Clicking a row opens a detail window (shopping list, sell-at/breakeven, unknown-recipe warning).
- [x] Filters matching CLI options: disciplines multi-select.
- [x] Timegated include toggle (Settings → persisted to `gw2-arbitrage.toml`, applied live via `config::INCLUDE_TIMEGATED`).
- [x] `--count` limit and `--include-ascended` toggle as runtime settings (Settings window → `config::COUNT_LIMIT` / `config::INCLUDE_ASCENDED` atomics + TOML persistence, applied on the next analysis run).
- [x] CSV export button (`--output-csv` equivalent) with a native save-file dialog, same columns as CLI output.
- [x] "Refresh cache / reset cache" button (`--reset-cache` equivalent) via `analysis::reset_data_files()` (deletes items/recipes data, keeps icons + favorites).

## Phase 2 — Item detail window
- [x] Clicking a row opens a detail window: icon (rarity-colored name), type, level, restrictions.
- [x] Shopping list for the item (reuse the `item-id` code path): ingredients, quantities, buy-from-TP vs vendor vs craft decisions, total cost, exact profit.
- [x] Show the TP order book (top 5 bids/asks, collapsible) in the detail window — `calc_item_profit` now also returns the crafted item's own `api::ItemListings`.
- [x] Links: gw2efficiency crafting calculator, GW2 wiki (`https://wiki.guildwars2.com/wiki/?search=…`), gw2bltc.
- [x] "You don't know this recipe" warning when the recipe id is missing from account unlocks (requires API key).

## Phase 3 — Favorites
- [x] Star/checkbox per row in the list.
- [x] Persist favorites as item ids in a JSON file (`favorites.json` in the cache dir) via new `favorites.rs` module.
- [x] Favorites pinned to the top of the list; sorting applies within favorites first, then the rest.
- [x] "Show favorites only" filter toggle (the "only" checkbox in the filter row).
- [x] Favorite toggle also available inside the detail window.

## Phase 4 — Windows polish
- [ ] `cargo build --release` on Windows (GitHub Actions already builds + uploads the exe as a run artifact on every push); produce a portable zip (single `.exe`) and/or an installer (e.g. Inno Setup / NSIS).
- [ ] High-DPI support. (App/window icon deliberately deferred for now — will be added later.)
- [x] Rarity colors (Junk grey → Legendary orange) for names in the list and detail window.
- [x] Remember GUI settings (filters: sort order, favorites-only, discipline selections) — persisted to `gui_prefs.json` in the cache dir.
- [x] Settings window: GW2 API key editing, saved to the existing TOML config file (`gw2-arbitrage.toml`).
- [ ] Friendly error dialogs (no internet, API key rejected, cache corrupt → suggest reset).
- [x] Hide the console window in GUI mode: the release binary is built with `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`, so double-clicking opens **only the GUI**. CLI mode still works (`--cli`, `--help`, `--version`, any option): `main.rs` calls `AttachConsole(ATTACH_PARENT_PROCESS)` (falling back to `AllocConsole`) before anything prints, so output appears in the terminal that launched it. Debug builds keep the console so `cargo run`/tests behave normally. Verified in CI by reading the PE subsystem field of the built exe (must be 2 = GUI, not 3 = console) in both `gui-ci.yml` and `rust.yml`.

## Phase 5 — Extras
- [ ] Auto-populate currency conversion values from `/v2/account/wallet` (API key).
- [x] Per-item "refresh prices now" in the detail window (bypasses the listings cache via `calc_item_profit(refresh: true)`).
- [ ] Material-bank awareness (`/v2/account/materials`) to compute profit using owned materials.
- [x] Sell-velocity columns (6h / 12h / 24h / 7d / 2w / 1m / 3m / 6m / 1y / 2y, units/day) fetched from `https://api.datawars2.ie/gw2/v2/history/…` (see new `src/velocity.rs`), with per-window coverage checks (≥80% of expected buckets, else "–"), 4 background worker threads, and sortable columns.
- [x] Table rework: clickable column headers sort the table (click again to flip direction), full-window width via `egui_extras::TableBuilder` (resizable columns, name column takes remaining space).
- [x] Settings → "Velocity windows": each window (6h → 2y) can be enabled/disabled; disabled windows hide their column. Choices are remembered in `gui_prefs.json`. Disabling **all** hourly (or all daily) windows skips that endpoint entirely per item; otherwise extra windows cost no extra requests, so there is no note about speed in the UI. Enabling a window later back-fills it without re-running the analysis.
- [x] Show velocity in the item detail window (per-window units/day for the enabled windows; on-demand fetch when the background workers have not reached that item yet).
- [x] Disk-cache velocity results (TTL ~1 day) so re-running the analysis doesn't refetch every item.
- [x] Settings → "Background worker threads" slider (**1–16, default 4**, persisted in `gui_prefs.json`, clamped on load so a hand-edited file cannot set 0 workers). Applies to velocity fetching and icon prefetch, and takes effect the next time workers are spawned. Max 16 chosen deliberately: each item only needs 1–2 requests and datawars2.ie publishes no rate limit, so beyond that the server becomes the bottleneck.
- [ ] Background auto-refresh of the list on a timer.
