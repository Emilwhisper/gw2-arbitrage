# Changelog

### v2.4.0

#### Features

* Charged Quartz Crystal as an optional synthetic recipe (25x Quartz Crystal, patient-aware pricing) behind `--include-charged-quartz` plus `--include-timegated`, so celestial-inscription chains such as the Celestial Pearl weapons can appear.
* Account currencies in the GUI Settings (Karma free toggle, Unbound/Volatile Magic and Research Notes enable toggles with copper-per-token rates), live via atomics and persisted to `gw2-arbitrage.toml`.
* Currency rates editable in the Filters window with snapshot recompute (no downloads) on Apply, first-enable community-estimate defaults (UM 3c, VM 30c, RN 250c), and a stale-list warning with a Rescan button.

#### Fixes

* Import the `Zero` trait in `config.rs` for the live currency helper.

### v2.3.5

#### Features

* Count limit defaults to 1 craft on first load (lists show one-craft economics out of the box; `--count 0` means no limit).
* Min-profit filter compares profit per craft instead of per item.

### v2.3.4

#### Fixes

* `--count` limits crafts (batches), not output units: a small limit no longer silently hides grouped recipes (e.g. `--count 1` crafts one full batch).

### v2.3.3

#### Fixes

* Item table body fills the available panel height instead of stopping at the default 800px scroll height (no more dead zone below the list in tall windows).

### v2.3.2

#### Fixes

* The detail filter "Min profit (per item, copper)" is now applied to the profit of one crafted unit instead of the batch total, so a large `--count` can no longer pass a threshold that the unit margin fails.
* A genuinely zero cost no longer renders as an empty string in the item window (it used to print `1 for  (@  each)` in the crafting tree); zero money prints as `0c`.
* Mystic Forge material promotions price the Mystic Binding Agent at one Bottle of Elonian Wine (2504c) rather than free, so their profits are no longer overstated.
* Crafting-tree leaf rows show the real vendor/token amount for vendor-bought ingredients (the purchase plan books them at zero because the price is charged in the crafting-cost recursion), so rows no longer display a misleading `0c`.
* The lazy config global no longer reads `std::env::args` implicitly: the binary seeds it explicitly, so `cargo test -- <filter>`, `--nocapture` and `--test-threads=1` work instead of crashing the test binary.
* Cache flushing only deletes `cache_`-prefixed names (a non-UTF-8 name used to be deleted anyway) and falls back to the modification time when the filesystem has no birth times.
* Order-book reservation counters use saturating decrements with debug asserts, so a future imbalance cannot wrap a `u32` and silently poison the liquidity simulation in release builds.
* The test suite is isolated from the developer's saved config file, and a regression test pins the Mystic Forge leg pricing.

### v2.3.1

#### Features

* Item window always opens showing a single crafted unit, with per-unit cost and profit lines.
* Craft quantity picker next to Refresh prices: exact recomputation for any chosen quantity, including at a loss, with a thin-book note on shortfall.
* Source selector options carry live unit prices (Buy @ ask, Sell @ bid, Craft estimate, Vendor price).

### v2.3.0

#### Features

* Instant vs Patient price mode (toolbar dropdown, `--patient` flag): buy at asks/sell at bids, or place orders at bids/asks and wait.
* Crafting tree in the item window (replaces the flat list): collapsible nodes, per-row Buy/Sell/Craft/Vendor pins with live unit prices, and an estimate panel.
* Mystic Forge material promotions (all tier upgrades) under their own filter; account-bound forge basics counted as free.
* Detail window always shows cost-to-make-1 and profit-per-1; links go straight to items; version in the window title.

#### Fixes

* Vendor leftover unit cost, purchase min/max tracking, unknown recipes shown by name, sell-range wording, endless icon repaint, crafting-steps guard.

### v2.2.1

#### Features

* Wider analysis levels: the right-click menu on Run analysis now offers Normal, -50s, -1g and -2g loss tolerances.
* Detail window links go straight to the item (wiki article, gw2efficiency calculator page, gw2bltc item page) instead of search pages.

### v2.2.0

#### Features

* Faster scans (warm full scan ~3.5 min to under a minute): gzip-compressed API responses, prices fetched only for recipe-relevant items, and repeat runs within 5 minutes recompute from an in-memory market snapshot with zero downloads.
* Scan progress counters (pages and batches) in the status line.

### v2.1.1

#### Fixes

* Survive transient GW2 API failures (CDN error pages, throttling) with retries, backoff and full error context instead of a contextless parse error killing the whole scan.
* Faster scans: shared HTTP client with connection reuse and parallel listing batches.

### v2.1.0

#### Features

* Filters window: min velocity (with window selector), min profit % and min profit in copper, applied on demand and remembered.
* Wider analysis mode (right-click "Run analysis"): keeps flips with total profit above -1g.
* Karma counts as free in profit math; Lime priced via Limes in Bulk, so recipes using it (e.g. Bowl of Prickly Pear Sorbet) are listed.

#### Fixes

* Settings Close button actually closes the window.
* Item table keeps full panel width so the horizontal scrollbar stays docked at the window edge.
* Solid scrollbars with a reserved gutter instead of tiny floating bars covering the last column.
* Wide simulation stops before the running total breaches -1g, so it stays a superset of the normal run.

### v0.6.2

#### Features

* Support --cache-dir option.

### v0.6.1

#### Fixes

* Handle page total changing during pagination.

## v0.6.0

### Features

* Cache files in system cache directory.

### Fixes

* Reduce cache size by removing unused fields.
