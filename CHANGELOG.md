# Changelog

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
