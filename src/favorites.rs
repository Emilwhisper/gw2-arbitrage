//! Persistence for favorite items.
//!
//! Favorites are stored as a JSON array of item ids in the cache directory
//! (`favorites.json`), so they survive cache resets and updates.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::CONFIG;

const FAVORITES_FILE: &str = "favorites.json";

fn favorites_path() -> PathBuf {
    CONFIG.cache_dir.join(FAVORITES_FILE)
}

/// Load the set of favorite item ids. Missing/corrupt file → empty set.
pub fn load() -> HashSet<u32> {
    std::fs::read_to_string(favorites_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Persist the set of favorite item ids.
pub fn save(favorites: &HashSet<u32>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(favorites).map_err(|e| e.to_string())?;
    std::fs::write(favorites_path(), json).map_err(|e| e.to_string())
}
