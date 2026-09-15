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
use egui::TextureHandle;

use crate::analysis::{self, Analysis};
use crate::api;
use crate::crafting;
use crate::favorites;
use crate::icons;
use crate::item::Item;
use crate::money::Money;
use crate::profit::ProfitableItem;
use crate::recipe::Recipe;

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
}

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
    favorites: HashSet<u32>,
    favorites_dirty: bool,
    icon_textures: HashMap<u32, Option<TextureHandle>>,
    pending_icons: HashSet<u32>,
    detail_item_id: Option<u32>,
    detail_open: bool,
    detail_loading: bool,
    detail: Option<DetailData>,
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
            favorites: favorites::load(),
            favorites_dirty: false,
            icon_textures: HashMap::new(),
            pending_icons: HashSet::new(),
            detail_item_id: None,
            detail_open: false,
            detail_loading: false,
            detail: None,
        }
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

    fn request_item_detail(&mut self, item_id: u32) {
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
            let result = runtime.block_on(analysis::run_item_analysis(&analysis, item_id, None));
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
            }
        }
    }
}


impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        self.flush_favorites();

        // keep repainting while background work is running
        if self.running || self.detail_loading || !self.pending_icons.is_empty() {
            ctx.request_repaint();
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.running, |ui| {
                    if ui.button("Run analysis").clicked() {
                        self.spawn_analysis();
                    }
                });
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
    }
}

impl App {
    fn show_item_list(&mut self, ui: &mut egui::Ui, analysis: &Analysis) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.sort_by_profit_desc, "Sort by profit (high → low)");
            ui.separator();
            ui.label(format!("{} items", self.profitable_items.len()));
        });
        ui.separator();

        // Precompute everything the UI closure needs so it doesn't borrow `self`.
        let mut items: Vec<&ProfitableItem> = self.profitable_items.iter().collect();
        if self.sort_by_profit_desc {
            items.sort_by_key(|i| {
                (!self.favorites.contains(&i.id), -i.profit.to_copper_value())
            });
        } else {
            items.sort_by_key(|i| !self.favorites.contains(&i.id));
        }
        let favorites = self.favorites.clone();
        let detail_item_id = self.detail_item_id;
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("item_grid")
                .striped(true)
                .num_columns(8)
                .show(ui, |ui| {
                    ui.strong("Icon");
                    ui.strong("★");
                    ui.strong("Name");
                    ui.strong("Disciplines");
                    ui.strong("Item ID");
                    ui.strong("Total profit");
                    ui.strong("Profit / item");
                    ui.strong("Profit / step");
                    ui.end_row();
                    for item in &items {
                        if item.count == 0 {
                            continue;
                        }
                        // icon (lazy download + cache)
                        match icon_ids.get(&item.id) {
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
                        }
                        // favorite star
                        let is_favorite = favorites.contains(&item.id);
                        if ui
                            .selectable_label(
                                is_favorite,
                                if is_favorite { "★" } else { "☆" },
                            )
                            .clicked()
                        {
                            favorite_toggled = Some(item.id);
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
                        if ui
                            .selectable_label(
                                detail_item_id == Some(item.id),
                                format!("{:<50}", name),
                            )
                            .clicked()
                        {
                            clicked = Some(item.id);
                        }
                        ui.label(disciplines);
                        ui.label(format!("{}", item.id));
                        ui.label(format!("{}", item.profit));
                        ui.label(format!("{}", item.profit_per_item()));
                        ui.label(format!("{}", item.profit_per_crafting_step()));
                        ui.end_row();
                    }
                });
        });

        if let Some(item_id) = clicked {
            self.request_item_detail(item_id);
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
        let analysis = match &self.analysis {
            Some(a) => a,
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
        ui.heading(item.to_string());
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
    }
}
