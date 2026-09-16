# Changelog

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
