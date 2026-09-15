//! Minimal egui/eframe GUI for gw2-arbitrage.
//!
//! Runs the analysis pipeline from `analysis.rs` on a background thread and
//! displays the profitable items in a list. Clicking an item opens a detail
//! window with its shopping list.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use egui::TextureHandle;

use crate::analysis::{self, Analysis};
use crate::api;
use crate::config;
use crate::crafting;
use crate::favorites;
use crate::icons;
use crate::item::{Item, Rarity};
use crate::money::Money;
use crate::profit::ProfitableItem;
use crate::recipe::Recipe;
use crate::velocity;

enum Event {
    Progress(String),
    AnalysisDone(Arc<Analysis>, Vec<ProfitableItem>),
    AnalysisError(String),
    ItemDone(
        u32,
        Option<ProfitableItem>,
        HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
        Vec<u32>,
        HashMap<u32, api::Price>,
    ),
    ItemError(u32, String),
    IconLoaded(u32, Option<PathBuf>),
    VelocityLoaded(u32, Option<velocity::Velocity>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SortColumn {
    Name,
    Disciplines,
    ItemId,
    TotalProfit,
    ProfitPerItem,
    ProfitPerStep,
    Vel6h,
    Vel12h,
    Vel24h,
    Vel7d,
    Vel2w,
    Vel1m,
    Vel3m,
}

impl SortColumn {
    /// Default direction when a column is first clicked.
    fn default_desc(self) -> bool {
        !matches!(self, SortColumn::Name | SortColumn::Disciplines)
    }

    fn header(self) -> &'static str {
        match self {
            SortColumn::Name => "Name",
            SortColumn::Disciplines => "Disciplines",
            SortColumn::ItemId => "Item ID",
            SortColumn::TotalProfit => "Total profit",
            SortColumn::ProfitPerItem => "Profit / item",
            SortColumn::ProfitPerStep => "Profit / step",
            SortColumn::Vel6h => "Vel 6h",
            SortColumn::Vel12h => "Vel 12h",
            SortColumn::Vel24h => "Vel 24h",
            SortColumn::Vel7d => "Vel 7d",
            SortColumn::Vel2w => "Vel 2w",
            SortColumn::Vel1m => "Vel 1m",
            SortColumn::Vel3m => "Vel 3m",
        }
    }
}

const ALL_SORT_COLUMNS: [SortColumn; 13] = [
    SortColumn::Name,
    SortColumn::Disciplines,
    SortColumn::ItemId,
    SortColumn::TotalProfit,
    SortColumn::ProfitPerItem,
    SortColumn::ProfitPerStep,
    SortColumn::Vel6h,
    SortColumn::Vel12h,
    SortColumn::Vel24h,
    SortColumn::Vel7d,
    SortColumn::Vel2w,
    SortColumn::Vel1m,
    SortColumn::Vel3m,
];

pub fn run() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 700.0])
            .with_title("gw2-arbitrage"),
        ..Default::default()
    };
    eframe::run_native(
        "gw2-arbitrage",
        options,
        Box::new(|_cc| Box::new(App::new())),
    )
}

struct App {
    events: Receiver<Event>,
    events_sender: Sender<Event>,
    analysis: Option<Arc<Analysis>>,
    profitable_items: Vec<ProfitableItem>,
    running: bool,
    status: String,
    sort_by_profit_desc: bool,
    favorites_only: bool,
    discipline_filter: Vec<config::Discipline>,
    favorites: HashSet<u32>,
    favorites_dirty: bool,
    icon_textures: HashMap<u32, Option<TextureHandle>>,
    pending_icons: HashSet<u32>,
    velocities: HashMap<u32, velocity::Velocity>,
    velocities_requested: HashSet<u32>,
    sort_column: SortColumn,
    sort_desc: bool,
    detail_item_id: Option<u32>,
    detail_open: bool,
    detail_loading: bool,
    detail: Option<DetailData>,
    show_settings: bool,
    api_key_input: String,
    prefs_dirty: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct GuiPrefs {
    sort_by_profit_desc: Option<bool>,
    favorites_only: Option<bool>,
    disciplines: Option<Vec<String>>,
}

const GUI_PREFS_FILE: &str = "gui_prefs.json";

fn load_gui_prefs() -> GuiPrefs {
    let path = crate::config::CONFIG.cache_dir.join(GUI_PREFS_FILE);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_gui_prefs(prefs: &GuiPrefs) -> Result<(), String> {
    let path = crate::config::CONFIG.cache_dir.join(GUI_PREFS_FILE);
    let json = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

type DetailData = (
    Option<ProfitableItem>,
    HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
    Vec<u32>,
    HashMap<u32, api::Price>,
);

impl App {
    fn new() -> Self {
        let (tx, rx) = channel();
        App {
            events: rx,
            events_sender: tx,
            analysis: None,
            profitable_items: vec![],
            running: false,
            status: "Ready. Click 'Run analysis' to fetch databases and compute profitable items."
                .to_string(),
            sort_by_profit_desc: true,
            favorites_only: false,
            discipline_filter: vec![],
            favorites: favorites::load(),
            favorites_dirty: false,
            icon_textures: HashMap::new(),
            pending_icons: HashSet::new(),
            velocities: HashMap::new(),
            velocities_requested: HashSet::new(),
            sort_column: SortColumn::TotalProfit,
            sort_desc: true,
            detail_item_id: None,
            detail_open: false,
            detail_loading: false,
            detail: None,
            show_settings: false,
            api_key_input: crate::config::CONFIG.api_key.clone().unwrap_or_default(),
            prefs_dirty: false,
        }
        .with_prefs(load_gui_prefs())
    }

    fn spawn_analysis(&mut self) {
        if self.running {
            return;
        }
        self.running = true;
        self.status = "Starting analysis...".to_string();
        self.profitable_items.clear();
        let tx = self.events_sender.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            let tx2 = tx.clone();
            let result: Result<(Arc<Analysis>, Vec<ProfitableItem>), String> = runtime
                .block_on(async {
                    let notify = move |url: &str| {
                        let _ = tx2.send(Event::Progress(format!("Fetching {}", url)));
                    };
                    let analysis = analysis::load_analysis(Some(&notify))
                        .await
                        .map_err(|e| e.to_string())?;
                    let analysis = Arc::new(analysis);
                    let items = analysis::run_list_analysis(&analysis, Some(&notify))
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok((analysis, items))
                });
            match result {
                Ok((analysis, items)) => {
                    let _ = tx.send(Event::AnalysisDone(analysis, items));
                }
                Err(e) => {
                    let _ = tx.send(Event::AnalysisError(e));
                }
            }
        });
    }

    fn request_item_detail(&mut self, item_id: u32, refresh: bool) {
        let analysis = match &self.analysis {
            Some(a) => Arc::clone(a),
            None => return,
        };
        self.detail_item_id = Some(item_id);
        self.detail_open = true;
        self.detail_loading = true;
        self.detail = None;
        let tx = self.events_sender.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            let result =
                runtime.block_on(analysis::run_item_analysis(&analysis, item_id, None, refresh));
            match result {
                Ok(data) => {
                    let _ = tx.send(Event::ItemDone(item_id, data.0, data.1, data.2, data.3));
                }
                Err(e) => {
                    let _ = tx.send(Event::ItemError(item_id, e.to_string()));
                }
            }
        });
    }

    fn request_icon(&mut self, ctx: &egui::Context, item_id: u32, icon_url: Option<String>) {
        if self.pending_icons.contains(&item_id) || self.icon_textures.contains_key(&item_id) {
            return;
        }
        self.pending_icons.insert(item_id);
        let tx = self.events_sender.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            let path = runtime.block_on(icons::get_icon(item_id, icon_url.as_deref(), None));
            let _ = tx.send(Event::IconLoaded(item_id, path));
            ctx.request_repaint();
        });
    }

    fn load_icon_texture(&mut self, ctx: &egui::Context, item_id: u32, path: Option<PathBuf>) {
        let texture = path.and_then(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let texture = ctx.load_texture(
                format!("icon-{}", item_id),
                egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                egui::TextureOptions::LINEAR,
            );
            Some(texture)
        });
        self.icon_textures.insert(item_id, texture);
    }

    fn toggle_favorite(&mut self, item_id: u32) {
        if !self.favorites.remove(&item_id) {
            self.favorites.insert(item_id);
        }
        self.favorites_dirty = true;
    }

    fn flush_favorites(&mut self) {
        if self.favorites_dirty {
            if let Err(e) = favorites::save(&self.favorites) {
                self.status = format!("Failed to save favorites: {}", e);
            }
            self.favorites_dirty = false;
        }
    }

    fn with_prefs(mut self, prefs: GuiPrefs) -> Self {
        if let Some(v) = prefs.sort_by_profit_desc {
            self.sort_by_profit_desc = v;
        }
        if let Some(v) = prefs.favorites_only {
            self.favorites_only = v;
        }
        if let Some(disciplines) = prefs.disciplines {
            self.discipline_filter = disciplines
                .iter()
                .filter_map(|d| d.parse::<config::Discipline>().ok())
                .collect();
        }
        self
    }

    fn flush_prefs(&mut self) {
        if self.prefs_dirty {
            let prefs = GuiPrefs {
                sort_by_profit_desc: Some(self.sort_by_profit_desc),
                favorites_only: Some(self.favorites_only),
                disciplines: Some(
                    self.discipline_filter
                        .iter()
                        .map(|d| d.to_string())
                        .collect(),
                ),
            };
            if let Err(e) = save_gui_prefs(&prefs) {
                self.status = format!("Failed to save GUI settings: {}", e);
            }
            self.prefs_dirty = false;
        }
    }

    fn save_api_key(&mut self) {
        let path = crate::config::CONFIG.config_file_path.clone();
        let key = self.api_key_input.trim().to_string();
        let result = (|| -> Result<(), String> {
            let mut table: toml::Value = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_else(|| toml::Value::Table(Default::default()));
            if let Some(t) = table.as_table_mut() {
                if key.is_empty() {
                    t.remove("api_key");
                } else {
                    t.insert("api_key".into(), toml::Value::String(key));
                }
            }
            let out = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, out).map_err(|e| e.to_string())
        })();
        self.status = match result {
            Ok(_) => format!(
                "Saved API key to {}. Restart the app to apply it.",
                path.display()
            ),
            Err(e) => format!("Failed to save config: {}", e),
        };
    }

    /// Kick off background workers that fetch sell-velocity data for every
    /// profitable item from datawars2.ie (one request per item; the API does
    /// not support multi-ID requests).
    fn spawn_velocity_workers(&mut self) {
        let ids: Vec<u32> = self
            .profitable_items
            .iter()
            .filter(|i| i.count > 0 && !self.velocities.contains_key(&i.id))
            .map(|i| i.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.velocities_requested
            .extend(ids.iter().copied());
        let queue = Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::from(ids),
        ));
        for _ in 0..4 {
            let tx = self.events_sender.clone();
            let queue = Arc::clone(&queue);
            thread::spawn(move || {
                let runtime =
                    tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                loop {
                    let id = queue.lock().expect("queue poisoned").pop_front();
                    let Some(id) = id else { break };
                    let v = runtime
                        .block_on(velocity::fetch_velocity(id))
                        .ok();
                    let _ = tx.send(Event::VelocityLoaded(id, v));
                }
            });
        }
    }

    fn export_csv(&mut self) {
        let analysis = match self.analysis.clone() {
            Some(a) => a,
            None => return,
        };
        let path = match rfd::FileDialog::new()
            .set_title("Export profitable items to CSV")
            .set_file_name("gw2-arbitrage.csv")
            .add_filter("CSV files", &["csv"])
            .save_file()
        {
            Some(p) => p,
            None => return,
        };

        let mut items: Vec<&ProfitableItem> = self.profitable_items.iter().collect();
        items.sort_by_key(|i| -i.profit.to_copper_value());

        let mut rows: Vec<Vec<String>> = vec![vec![
            "Name",
            "Disciplines",
            "Item ID",
            "Unknown recipes",
            "Total profit",
            "No. required",
            "Profit / item",
            "Crafting steps",
            "Profit / step",
            "Profit on cost",
        ]
        .into_iter()
        .map(String::from)
        .collect()];

        for item in items {
            if item.count == 0 {
                continue;
            }
            let name = analysis
                .items_map
                .get(&item.id)
                .map_or_else(|| "???".to_string(), |i| i.to_string());
            let disciplines = analysis
                .recipes_map
                .get(&item.id)
                .map(|r: &Recipe| {
                    r.disciplines
                        .iter()
                        .map(|d| d.get_abbrev())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            let unknown = item
                .crafted_items
                .unknown_recipes(&analysis.recipes_map, &analysis.known_recipes)
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<String>>()
                .join(",");
            rows.push(vec![
                name,
                disciplines,
                item.id.to_string(),
                unknown,
                item.profit.to_string(),
                item.count.to_string(),
                item.profit_per_item().to_copper_value().to_string(),
                item.crafting_steps.to_string(),
                item.profit_per_crafting_step().to_copper_value().to_string(),
                ((item.profit_on_cost() * 100_f64).round() as i64).to_string(),
            ]);
        }

        self.status = format!("Exporting CSV to {}", path.display());
        thread::spawn(move || {
            let result: Result<(), String> = (|| {
                let mut writer = csv::Writer::from_path(&path).map_err(|e| e.to_string())?;
                for row in &rows {
                    writer.write_record(row).map_err(|e| e.to_string())?;
                }
                writer.flush().map_err(|e| e.to_string())
            })();
            if let Err(e) = result {
                eprintln!("CSV export failed: {}", e);
            }
        });
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Progress(msg) => self.status = msg,
                Event::AnalysisDone(analysis, items) => {
                    self.analysis = Some(analysis);
                    self.profitable_items = items;
                    self.running = false;
                    self.status =
                        format!("Done: {} profitable items", self.profitable_items.len());
                    self.spawn_velocity_workers();
                }
                Event::AnalysisError(e) => {
                    self.running = false;
                    self.status = format!("Analysis failed: {}", e);
                }
                Event::ItemDone(item_id, profitable_item, purchased, unknown, prices) => {
                    if self.detail_item_id == Some(item_id) {
                        self.detail = Some((profitable_item, purchased, unknown, prices));
                        self.detail_loading = false;
                    }
                }
                Event::ItemError(item_id, e) => {
                    if self.detail_item_id == Some(item_id) {
                        self.detail_loading = false;
                        self.status = format!("Item analysis failed: {}", e);
                    }
                }
                Event::IconLoaded(item_id, path) => {
                    self.load_icon_texture(ctx, item_id, path);
                }
                Event::VelocityLoaded(item_id, v) => {
                    self.velocities_requested.remove(&item_id);
                    if let Some(v) = v {
                        self.velocities.insert(item_id, v);
                    }
                }
            }
        }
    }
}


/// GW2 rarity colors.
fn rarity_color(rarity: &Rarity) -> egui::Color32 {
    match rarity {
        Rarity::Junk => egui::Color32::from_rgb(0xAA, 0xAA, 0xAA),
        Rarity::Basic => egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
        Rarity::Fine => egui::Color32::from_rgb(0x62, 0xA4, 0xDA),
        Rarity::Masterwork => egui::Color32::from_rgb(0x33, 0xCC, 0x33),
        Rarity::Rare => egui::Color32::from_rgb(0xF6, 0xD6, 0x4A),
        Rarity::Exotic => egui::Color32::from_rgb(0xBA, 0x5C, 0xFF),
        Rarity::Ascended => egui::Color32::from_rgb(0xFB, 0x3E, 0x8D),
        Rarity::Legendary => egui::Color32::from_rgb(0xFF, 0x84, 0x00),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        self.flush_favorites();
        self.flush_prefs();

        // keep repainting while background work is running
        if self.running || self.detail_loading || !self.pending_icons.is_empty() || !self.velocities_requested.is_empty() {
            ctx.request_repaint();
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.running, |ui| {
                    if ui.button("Run analysis").clicked() {
                        self.spawn_analysis();
                    }
                    if ui.button("Reset cache & re-run").clicked() {
                        analysis::reset_data_files();
                        self.spawn_analysis();
                    }
                });
                ui.add_enabled_ui(
                    !self.profitable_items.is_empty() && self.analysis.is_some(),
                    |ui| {
                        if ui.button("Export CSV...").clicked() {
                            self.export_csv();
                        }
                    },
                );
                ui.separator();
                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }
                if self.running {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let analysis = self.analysis.clone();
            if let Some(analysis) = analysis {
                self.show_item_list(ui, &analysis);
            } else if !self.running {
                ui.centered_and_justified(|ui| {
                    ui.label("Click 'Run analysis' to load the item and recipe databases.");
                });
            }
        });

        if self.detail_open {
            let item_id = self.detail_item_id.unwrap_or(0);
            let mut open = self.detail_open;
            egui::Window::new(format!("Item {}", item_id))
                .open(&mut open)
                .default_width(700.0)
                .show(ctx, |ui| {
                    self.show_item_detail(ui, item_id);
                });
            self.detail_open = open;
        }

        if self.show_settings {
            let mut open = self.show_settings;
            egui::Window::new("Settings")
                .open(&mut open)
                .default_width(500.0)
                .show(ctx, |ui| {
                    ui.label("Guild Wars 2 API key (needs the \"unlocks\" scope).");
                    ui.label("Enables the \"you may not know these recipes\" warnings.");
                    ui.add_space(4.0);
                    ui.text_edit_singleline(&mut self.api_key_input);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.save_api_key();
                        }
                        if ui.button("Close").clicked() {
                            self.show_settings = false;
                        }
                    });
                    ui.separator();
                    ui.small(format!(
                        "Config file: {}",
                        crate::config::CONFIG.config_file_path.display()
                    ));
                });
            self.show_settings = open;
        }
    }
}

impl App {
    fn show_item_list(&mut self, ui: &mut egui::Ui, analysis: &Analysis) {
        // filters
        ui.horizontal(|ui| {
            let mut favs_only = self.favorites_only;
            ui.checkbox(&mut favs_only, "★ only");
            if favs_only != self.favorites_only {
                self.favorites_only = favs_only;
                self.prefs_dirty = true;
            }
            ui.separator();
            ui.label("Disciplines:");
            for variant in [
                "Artificer",
                "Armorsmith",
                "Chef",
                "Huntsman",
                "Jeweler",
                "Leatherworker",
                "Tailor",
                "Weaponsmith",
                "Scribe",
                "Homesteader",
                "Achievement",
            ] {
                let Some(discipline) = variant.parse::<config::Discipline>().ok() else { continue };
                let mut checked = self.discipline_filter.contains(&discipline);
                if ui.checkbox(&mut checked, variant).changed() {
                    if checked {
                        self.discipline_filter.push(discipline);
                    } else {
                        self.discipline_filter.retain(|d| *d != discipline);
                    }
                    self.prefs_dirty = true;
                }
            }
            ui.separator();
            if !self.velocities_requested.is_empty() {
                ui.spinner();
                ui.label(format!("loading velocity ({}/{})…", self.velocities.len(), self.velocities.len() + self.velocities_requested.len()));
            }
        });
        ui.separator();

        // Precompute everything the UI closure needs so it doesn't borrow `self`.
        let favorites = self.favorites.clone();
        let detail_item_id = self.detail_item_id;
        let sort_column = self.sort_column;
        let sort_desc = self.sort_desc;
        let velocities = self.velocities.clone();
        let mut items: Vec<&ProfitableItem> = self
            .profitable_items
            .iter()
            .filter(|i| !self.favorites_only || self.favorites.contains(&i.id))
            .filter(|i| {
                self.discipline_filter.is_empty()
                    || analysis.recipes_map.get(&i.id).is_some_and(|r| {
                        r.disciplines.iter().any(|d| self.discipline_filter.contains(d))
                    })
            })
            .filter(|i| i.count > 0)
            .collect();

        let item_info =
            |analysis: &Analysis, id: u32| -> (String, String) {
                let name = analysis
                    .items_map
                    .get(&id)
                    .map_or_else(|| "???".to_string(), |i| i.to_string());
                let disciplines = analysis
                    .recipes_map
                    .get(&id)
                    .map(|r: &Recipe| {
                        r.disciplines
                            .iter()
                            .map(|d| d.get_abbrev())
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_default();
                (name, disciplines)
            };

        let sort_key = |i: &ProfitableItem| -> (f64, String) {
            let (name, disciplines) = item_info(analysis, i.id);
            match sort_column {
                SortColumn::Name => (0.0, name.to_lowercase()),
                SortColumn::Disciplines => (0.0, disciplines),
                SortColumn::ItemId => (i.id as f64, String::new()),
                SortColumn::TotalProfit => (i.profit.to_copper_value() as f64, String::new()),
                SortColumn::ProfitPerItem => {
                    (i.profit_per_item().to_copper_value() as f64, String::new())
                }
                SortColumn::ProfitPerStep => (
                    i.profit_per_crafting_step().to_copper_value() as f64,
                    String::new(),
                ),
                SortColumn::Vel6h => (velocities.get(&i.id).and_then(|v| v.h6).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel12h => (velocities.get(&i.id).and_then(|v| v.h12).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel24h => (velocities.get(&i.id).and_then(|v| v.h24).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel7d => (velocities.get(&i.id).and_then(|v| v.d7).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel2w => (velocities.get(&i.id).and_then(|v| v.w2).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel1m => (velocities.get(&i.id).and_then(|v| v.m1).unwrap_or(f64::NEG_INFINITY), String::new()),
                SortColumn::Vel3m => (velocities.get(&i.id).and_then(|v| v.m3).unwrap_or(f64::NEG_INFINITY), String::new()),
            }
        };

        items.sort_by(|a, b| {
            // favorites are always pinned to the top
            let fav = (!favorites.contains(&a.id)).cmp(&(!favorites.contains(&b.id)));
            let ka = sort_key(a);
            let kb = sort_key(b);
            let value = ka
                .0
                .partial_cmp(&kb.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(ka.1.cmp(&kb.1));
            let value = if sort_desc {
                value.reverse()
            } else {
                value
            };
            fav.then(value)
        });

        // texture ids are cheap to copy; None marks "failed to load"
        let icon_ids: HashMap<u32, Option<egui::TextureId>> = self
            .icon_textures
            .iter()
            .map(|(id, tex)| (*id, tex.as_ref().map(|t| t.id())))
            .collect();
        let icon_urls: HashMap<u32, Option<String>> = analysis
            .items_map
            .iter()
            .map(|(id, item)| (*id, item.icon.clone()))
            .collect();

        let mut clicked: Option<u32> = None;
        let mut favorite_toggled: Option<u32> = None;
        let mut icons_requested: Vec<u32> = vec![];
        let mut sort_changed: Option<(SortColumn, bool)> = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::remainder())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .header(26.0, |mut header| {
                header.col(|ui| {
                    ui.strong("Icon");
                });
                header.col(|ui| {
                    ui.strong("\u{2605}");
                });
                for col in ALL_SORT_COLUMNS {
                    header.col(|ui| {
                        let is_active = sort_column == col;
                        let arrow = if is_active {
                            if sort_desc { " \u{25bc}" } else { " \u{25b2}" }
                        } else {
                            ""
                        };
                        if ui
                            .button(
                                egui::RichText::new(format!("{}{}", col.header(), arrow)).strong(),
                            )
                            .clicked()
                        {
                            let new_desc = if is_active {
                                !sort_desc
                            } else {
                                col.default_desc()
                            };
                            sort_changed = Some((col, new_desc));
                        }
                    });
                }
            })
            .body(|mut body| {
                for item in &items {
                    body.row(28.0, |mut row| {
                        // icon (lazy download + cache)
                        row.col(|ui| match icon_ids.get(&item.id) {
                            Some(Some(tex_id)) => {
                                ui.image((*tex_id, egui::vec2(24.0, 24.0)));
                            }
                            Some(None) => {
                                ui.label("");
                            }
                            None => {
                                ui.label("");
                                icons_requested.push(item.id);
                            }
                        });
                        // favorite star
                        row.col(|ui| {
                            let is_favorite = favorites.contains(&item.id);
                            if ui
                                .selectable_label(
                                    is_favorite,
                                    if is_favorite { "\u{2605}" } else { "\u{2606}" },
                                )
                                .clicked()
                            {
                                favorite_toggled = Some(item.id);
                            }
                        });
                        let (name, disciplines) = item_info(analysis, item.id);
                        let name_color = analysis
                            .items_map
                            .get(&item.id)
                            .map(|i| rarity_color(i.rarity()))
                            .unwrap_or(egui::Color32::PLACEHOLDER);
                        row.col(|ui| {
                            if ui
                                .selectable_label(
                                    detail_item_id == Some(item.id),
                                    egui::RichText::new(name).color(name_color),
                                )
                                .clicked()
                            {
                                clicked = Some(item.id);
                            }
                        });
                        row.col(|ui| {
                            ui.label(disciplines);
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", item.id));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", item.profit));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", item.profit_per_item()));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", item.profit_per_crafting_step()));
                        });
                        // velocity columns (units/day); "\u{2026}" = loading, "\u{2013}" = no data
                        let v = velocities.get(&item.id);
                        let loading = !velocities.contains_key(&item.id);
                        for value in [
                            v.and_then(|v| v.h6),
                            v.and_then(|v| v.h12),
                            v.and_then(|v| v.h24),
                            v.and_then(|v| v.d7),
                            v.and_then(|v| v.w2),
                            v.and_then(|v| v.m1),
                            v.and_then(|v| v.m3),
                        ] {
                            row.col(|ui| match value {
                                Some(x) => {
                                    ui.label(format!("{:.1}", x));
                                }
                                None if loading => {
                                    ui.label("\u{2026}");
                                }
                                None => {
                                    ui.label("\u{2013}");
                                }
                            });
                        }
                    });
                }
            });

        if let Some((col, desc)) = sort_changed {
            self.sort_column = col;
            self.sort_desc = desc;
        }


        if let Some(item_id) = clicked {
            self.request_item_detail(item_id, false);
        }
        if let Some(item_id) = favorite_toggled {
            self.toggle_favorite(item_id);
        }
        for item_id in icons_requested {
            let icon_url = icon_urls.get(&item_id).cloned().flatten();
            self.request_icon(ui.ctx(), item_id, icon_url);
        }
    }

    fn show_item_detail(&mut self, ui: &mut egui::Ui, item_id: u32) {
        let analysis: Arc<Analysis> = match &self.analysis {
            Some(a) => Arc::clone(a),
            None => {
                ui.label("No analysis loaded");
                return;
            }
        };
        let item: &Item = match analysis.items_map.get(&item_id) {
            Some(i) => i,
            None => {
                ui.label("Item not found");
                return;
            }
        };
        let icon_url = item.icon.clone();
        let item_name = item.to_string();
        let name_color = rarity_color(item.rarity());
        let is_favorite = self.favorites.contains(&item_id);
        ui.horizontal(|ui| {
            if let Some(tex) = self.icon_textures.get(&item_id).and_then(|t| t.as_ref()) {
                ui.image((tex.id(), egui::vec2(32.0, 32.0)));
            }
            ui.heading(egui::RichText::new(&item_name).color(name_color));
            if ui
                .selectable_label(is_favorite, if is_favorite { "★" } else { "☆" })
                .clicked()
            {
                self.toggle_favorite(item_id);
            }
        });
        if !self.icon_textures.contains_key(&item_id) {
            self.request_icon(ui.ctx(), item_id, icon_url);
        }
        ui.horizontal(|ui| {
            let name_urlencoded = item.name.replace(' ', "%20");
            ui.hyperlink_to(
                "Wiki",
                format!("https://wiki.guildwars2.com/wiki/Special:Search?search={}", name_urlencoded),
            );
            ui.hyperlink_to(
                "gw2efficiency",
                format!(
                    "https://gw2efficiency.com/crafting/calculator/#g=1&q={}",
                    name_urlencoded
                ),
            );
            ui.hyperlink_to(
                "gw2bltc",
                format!("https://www.gw2bltc.com/en/tp/search?q={}&p=item", name_urlencoded),
            );
        });
        ui.separator();

        if self.detail_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Computing shopping list...");
            });
            return;
        }
        let data = match &self.detail {
            Some(d) => d,
            None => {
                ui.label("No data");
                return;
            }
        };
        let (profitable_item, purchased_ingredients, required_unknown_recipes, _prices) = data;

        let profitable_item = match profitable_item {
            Some(pi) => pi,
            None => {
                ui.label("Item is not profitable to craft");
                return;
            }
        };

        ui.label(format!(
            "{} x {} = {} profit ({} / step, {}%)",
            profitable_item.count,
            item.to_string(),
            Money::from_copper(profitable_item.profit.to_copper_value()),
            profitable_item.profit_per_crafting_step().to_copper_value(),
            (profitable_item.profit_on_cost() * 100_f64).round(),
        ));
        let price_msg = if profitable_item.max_sell == profitable_item.min_sell {
            format!("{}", profitable_item.min_sell)
        } else {
            format!(
                "{} to {}",
                profitable_item.max_sell, profitable_item.min_sell,
            )
        };
        ui.label(format!(
            "Sell at: {}, Money Required: {}, Breakeven price: {}",
            price_msg,
            profitable_item.crafting_cost.increase_by_listing_fee(),
            profitable_item.breakeven,
        ));

        if !required_unknown_recipes.is_empty() {
            ui.separator();
            ui.colored_label(egui::Color32::RED, "WARNING: you may not know these recipes:");
            ui.label(format!("{:?}", required_unknown_recipes));
        }

        ui.separator();
        let mut refresh_requested = false;
        ui.horizontal(|ui| {
            if ui.button("Refresh prices").clicked() {
                refresh_requested = true;
            }
            if self.detail_loading {
                ui.spinner();
            }
        });

        ui.strong("Shopping list");
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("shopping_grid")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    ui.strong("Source");
                    ui.strong("Ingredient");
                    ui.strong("Count");
                    ui.strong("Unit cost");
                    ui.strong("Total cost");
                    ui.end_row();
                    for ((ingredient_id, source), ingredient) in purchased_ingredients {
                        let name = analysis
                            .items_map
                            .get(ingredient_id)
                            .map_or_else(|| "???".to_string(), |i| i.to_string());
                        ui.label(format!("{:?}", source));
                        ui.label(name);
                        ui.label(format!("{}", ingredient.count));
                        ui.label(format!("{}", ingredient.min_price));
                        ui.label(format!("{}", ingredient.total_cost));
                        ui.end_row();
                    }
                });
        });

        if refresh_requested {
            self.request_item_detail(item_id, true);
        }
    }
}


