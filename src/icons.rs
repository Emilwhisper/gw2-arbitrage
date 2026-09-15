use std::path::{Path, PathBuf};

use crate::config::CONFIG;

/// Return the expected cache path for an item's icon PNG.
pub fn icon_path(icons_dir: &Path, item_id: u32) -> PathBuf {
    let mut path = icons_dir.to_path_buf();
    path.push(format!("{}.png", item_id));
    path
}

/// Get an item icon, using the local cache in `icons_dir` when available.
/// Downloads the PNG from `icon_url` on cache miss and stores it permanently.
/// Returns `None` if no icon URL is known or the download fails (the GUI
/// should display a placeholder).
pub async fn get_icon(
    item_id: u32,
    icon_url: Option<&str>,
    notify: Option<&dyn Fn(&str)>,
) -> Option<PathBuf> {
    let icon_url = icon_url?;
    let icons_dir = &CONFIG.icons_dir;

    let cache_path = icon_path(icons_dir, item_id);
    if cache_path.is_file() {
        return Some(cache_path);
    }

    if let Some(notify) = notify {
        notify(&format!("Fetching icon {}", icon_url));
    }

    let response = reqwest::get(icon_url).await.ok()?;
    if !response.status().is_success() {
        eprintln!(
            "Failed to fetch icon for item {} ({})",
            item_id,
            response.status()
        );
        return None;
    }
    let bytes = response.bytes().await.ok()?;

    // ensure the icons dir exists (created lazily; parent dirs are the cache dir)
    if let Err(e) = std::fs::create_dir_all(icons_dir) {
        eprintln!("Failed to create icon cache dir: {}", e);
        return None;
    }

    let tmp_path = cache_path.with_extension("png.tmp");
    if std::fs::write(&tmp_path, &bytes).is_err() {
        return None;
    }
    // rename for atomic-ish writes (concurrent requests for the same icon)
    if std::fs::rename(&tmp_path, &cache_path).is_err() {
        // someone else may have created it first; ignore if the final file exists
        let _ = std::fs::remove_file(&tmp_path);
        if !cache_path.is_file() {
            return None;
        }
    }

    Some(cache_path)
}
