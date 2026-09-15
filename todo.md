# todo.md — GUI Roadmap

Phased plan to add a Windows-first GUI to `gw2-arbitrage` while keeping the CLI mode fully functional.

## Phase 0 — Refactor for GUI readiness
- [ ] Retain the `icon` URL in `Item` (from `/v2/items`) instead of dropping it in the `ApiItem → Item` conversion (`#[serde(default)]` so old cached item DBs still parse; may require a one-time items re-download / cache bump).
- [ ] Extract data loading + profit pipeline from `main.rs` into library functions returning structured data (e.g. `Analysis { items, recipes, profitable_items }`) instead of printing directly.
- [ ] Reduce reliance on the global `CONFIG` (pass `CraftingOptions` / settings as parameters where practical), or document that the GUI must initialize `CONFIG` at startup.
- [x] Add `--cli` flag / argument detection so the single binary can run in either console or GUI mode. (No args or `--cli` absent logic: no args → GUI; `--cli` or any arguments → CLI.)
- [x] Icon caching: new `icons.rs` library module — `get_icon(item_id, url, notify) -> Option<PathBuf>` (check disk → download PNG → save → return path). Uses `CONFIG.icons_dir` (`<cache-dir>\icons`).
  - [ ] Store raw PNGs in a permanent `icons/` subfolder of the cache dir (`...\gw2-arbitrage\icons\<item_id>.png`); icons are immutable, no expiry, and `flush_cache` only deletes `cache_*` files so they survive `--reset-cache`.
  - [x] GUI renders item icons from the `icons.rs` disk cache: lazy per-row download on a background thread, decoded to egui textures, placeholder on failure.
  - [ ] In-memory LRU/HashMap of decoded images so egui doesn't re-read files per frame.
  - [ ] Optional "prefetch all icons" button using the existing parallel `stream::buffered` pattern.
  - [ ] CLI mode ignores icons entirely (no impact on console output).
- [ ] Add shopping-list rendering as a function returning data (not just `println!`), so the detail window can reuse it.

## Phase 1 — GUI framework and basic window
- [x] Choose framework. → **egui/eframe 0.27** (pure Rust, lightweight, great Windows support).
- [x] Basic window: "Run analysis" button with progress indicator (background tokio task + mpsc channel).
- [x] Show the profitable-items list in a sortable table: name, disciplines, item id, total profit, profit/item, profit/step.
- [x] Clicking a row opens a detail window (shopping list, sell-at/breakeven, unknown-recipe warning).
- [ ] Filters matching CLI options: disciplines multi-select, `--count` limit, timegated/ascended include toggles.
- [x] CSV export button (`--output-csv` equivalent) with a native save-file dialog, same columns as CLI output.
- [x] "Refresh cache / reset cache" button (`--reset-cache` equivalent) via `analysis::reset_data_files()` (deletes items/recipes data, keeps icons + favorites).

## Phase 2 — Item detail window
- [ ] Clicking a row opens a detail window: icon (rarity-colored name), type, level, restrictions.
- [ ] Shopping list for the item (reuse the `item-id` code path): ingredients, quantities, buy-from-TP vs vendor vs craft decisions, total cost, exact profit.
- [ ] Show full TP order book (top bids/asks) for the item if useful.
- [ ] Links: gw2efficiency crafting calculator, GW2 wiki (`https://wiki.guildwars2.com/wiki/?search=…`), gw2bltc.
- [ ] "You don't know this recipe" warning when the recipe id is missing from account unlocks (requires API key).

## Phase 3 — Favorites
- [x] Star/checkbox per row in the list.
- [x] Persist favorites as item ids in a JSON file (`favorites.json` in the cache dir) via new `favorites.rs` module.
- [x] Favorites pinned to the top of the list; sorting applies within favorites first, then the rest.
- [ ] "Show favorites only" filter toggle.
- [x] Favorite toggle also available inside the detail window.

## Phase 4 — Windows polish
- [ ] `cargo build --release` on Windows; produce a portable zip (single `.exe`) and/or an installer (e.g. Inno Setup / NSIS).
- [ ] App icon, window icon, high-DPI support.
- [ ] Rarity colors (Junk grey → Legendary orange) for names/icons.
- [ ] Remember GUI settings (window size, last filters) — extend the existing TOML config file.
- [ ] Friendly error dialogs (no internet, API key rejected, cache corrupt → suggest reset).

## Phase 5 — Extras
- [ ] Auto-populate currency conversion values from `/v2/account/wallet` (API key).
- [ ] Per-item "refresh prices now" in the detail window.
- [ ] Material-bank awareness (`/v2/account/materials`) to compute profit using owned materials.
- [ ] Background auto-refresh of the list on a timer.
