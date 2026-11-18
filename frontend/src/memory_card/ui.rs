use super::card::SlotInfo;
use super::icon::decode_icon;
use super::icon_png::{encode_icon_png, remap_icon_palette, IconPalette};
use super::io::{
    copy_card, default_source_dir, duckstation_dir, is_multi_disc, load_card, local_copy_name,
    replace_with_backup, retroarch_dir, scan_cards, write_new_file_atomically, CardFormat,
    EmulatorKind, LoadedCard,
};
use crate::filebrowser::{BrowseKind, BrowseResult, FileBrowser};
use crate::libretro::host::MessageKind;
use crate::theme::{self, UiTheme};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Local,
    Import,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowsePurpose {
    OpenLocal,
    ImportSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalAction {
    Convert,
    UseAsImportSource,
    ImportSourceBrowse,
    Delete,
    ExportIcon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceAction {
    CopyCard,
    ImportSave,
}

#[derive(Debug)]
enum PendingAction {
    Delete {
        target: PathBuf,
        save_name: String,
    },
    Import {
        target: PathBuf,
        source: PathBuf,
        save_name: String,
        overwrite: bool,
    },
    Copy {
        source: PathBuf,
        destination: PathBuf,
    },
}

struct CardRow {
    path: PathBuf,
    format: CardFormat,
    saves: Option<usize>,
    free: Option<usize>,
    error: Option<String>,
}

impl CardRow {
    fn from_path(path: PathBuf) -> Self {
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let fallback_format = if extension == "gme" {
            CardFormat::Gme
        } else {
            CardFormat::Raw
        };
        match load_card(&path) {
            Ok(card) => Self {
                path,
                format: card.format,
                saves: Some(card.summary.saves.len()),
                free: Some(card.summary.free_slots),
                error: None,
            },
            Err(error) => Self {
                path,
                format: fallback_format,
                saves: None,
                free: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    fn label(&self) -> String {
        match (self.saves, self.free, &self.error) {
            (Some(saves), Some(free), None) => format!(
                "{}  ·  {}  ·  {} save(s), {} free",
                self.name(),
                self.format.short(),
                saves,
                free
            ),
            _ => format!("{}  ·  {}  ·  unreadable", self.name(), self.format.short()),
        }
    }
}

pub struct MemoryCardManager {
    pub open: bool,
    pub local_dir: PathBuf,
    pub source_dir: PathBuf,
    pub source_dir_text: String,
    pub source_dir_dirty: bool,
    tab: Tab,
    cards: Vec<CardRow>,
    selected_card: Option<usize>,
    loaded: Option<LoadedCard>,
    selected_save: Option<usize>,
    source_cards: Vec<CardRow>,
    source_selected: Option<usize>,
    source_loaded: Option<LoadedCard>,
    source_save_selected: Option<usize>,
    convert_name: String,
    copy_name: String,
    icon_name: String,
    icon_scale: usize,
    icon_circle: bool,
    icon_palette: Option<IconPalette>,
    icon_texture: Option<egui::TextureHandle>,
    icon_texture_key: Option<String>,
    error: Option<String>,
    status: Option<(String, MessageKind)>,
    file_browser: Option<FileBrowser>,
    browse_purpose: Option<BrowsePurpose>,
    pending: Option<(PendingAction, String)>,
}

impl MemoryCardManager {
    pub fn new(local_dir: PathBuf, source_dir: PathBuf) -> Self {
        Self {
            open: false,
            local_dir,
            source_dir_text: source_dir.display().to_string(),
            source_dir,
            source_dir_dirty: false,
            tab: Tab::Local,
            cards: Vec::new(),
            selected_card: None,
            loaded: None,
            selected_save: None,
            source_cards: Vec::new(),
            source_selected: None,
            source_loaded: None,
            source_save_selected: None,
            convert_name: String::new(),
            copy_name: String::new(),
            icon_name: String::new(),
            icon_scale: 4,
            icon_circle: false,
            icon_palette: None,
            icon_texture: None,
            icon_texture_key: None,
            error: None,
            status: None,
            file_browser: None,
            browse_purpose: None,
            pending: None,
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.error = None;
        self.status = None;
        self.tab = Tab::Local;
        if self.source_dir.as_os_str().is_empty() {
            let detected = default_source_dir();
            if !detected.as_os_str().is_empty() {
                self.source_dir_text = detected.display().to_string();
                self.source_dir = detected;
                self.source_dir_dirty = true;
            }
        }
        self.refresh_local();
        self.refresh_source();
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &UiTheme) -> Option<(String, MessageKind)> {
        if !self.open {
            return None;
        }

        let mut open = true;
        let mut header_close = false;
        let window_frame = theme::window_frame(&ctx.global_style(), theme);
        egui::Window::new("Memory Card Manager")
            .id(egui::Id::new("memory-card-manager"))
            .open(&mut open)
            .title_bar(false)
            .frame(window_frame)
            .resizable(true)
            .default_width(1280.0)
            .default_height(760.0)
            .show(ctx, |ui| {
                if theme::window_header(ui, "Memory Card Manager", theme) {
                    header_close = true;
                }
                theme::window_body(ui, |ui| {
                    self.body(ui, ctx, theme);
                });
            });

        if header_close {
            open = false;
        }
        self.open = open;

        self.poll_file_browser(ctx, theme);
        self.show_confirm(ctx);
        self.status.take()
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &UiTheme) {
        self.toolbar(ui, theme);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.tab == Tab::Local, "Local cards")
                .clicked()
            {
                self.tab = Tab::Local;
            }
            if ui
                .selectable_label(self.tab == Tab::Import, "Import & copy")
                .clicked()
            {
                self.tab = Tab::Import;
            }
        });
        ui.separator();

        match self.tab {
            Tab::Local => self.local_body(ui, ctx, theme),
            Tab::Import => self.import_body(ui, ctx, theme),
        }

        if let Some(error) = &self.error {
            ui.add_space(6.0);
            ui.colored_label(theme.error, error);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, _theme: &UiTheme) {
        let mut open_folder = false;
        let mut refresh_local = false;
        let mut source_changed = false;
        let mut scan_source = false;
        let mut use_duckstation = false;
        let mut use_retroarch = false;

        ui.horizontal(|ui| {
            ui.label("Local cards:");
            ui.monospace(self.local_dir.display().to_string());
            if ui.button("Open folder").clicked() {
                open_folder = true;
            }
            if ui.button("Refresh").clicked() {
                refresh_local = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Import from:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.source_dir_text)
                        .desired_width(520.0)
                        .hint_text("DuckStation memcards or RetroArch saves directory"),
                )
                .changed()
            {
                source_changed = true;
            }
            if ui.button("Scan").clicked() {
                scan_source = true;
            }
            if ui.button("DuckStation").clicked() {
                use_duckstation = true;
            }
            if ui.button("RetroArch").clicked() {
                use_retroarch = true;
            }
        });

        if source_changed {
            self.source_dir = PathBuf::from(self.source_dir_text.trim());
            self.source_dir_dirty = true;
        }
        if open_folder {
            let _ = std::process::Command::new("xdg-open")
                .arg(&self.local_dir)
                .spawn();
        }
        if refresh_local {
            self.refresh_local();
        }
        if use_duckstation {
            self.source_dir = duckstation_dir();
            self.source_dir_text = self.source_dir.display().to_string();
            self.source_dir_dirty = true;
            scan_source = true;
        }
        if use_retroarch {
            self.source_dir = retroarch_dir();
            self.source_dir_text = self.source_dir.display().to_string();
            self.source_dir_dirty = true;
            scan_source = true;
        }
        if scan_source {
            self.refresh_source();
        }

        if self.source_dir.as_os_str().is_empty() {
            ui.weak(
                "Choose a DuckStation or RetroArch memory-card directory, or use “Open source card…”.",
            );
        } else {
            ui.weak(format!(
                "Source: {} · {} card(s)",
                EmulatorKind::detect(&self.source_dir).name(),
                self.source_cards.len()
            ));
        }
    }

    fn local_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &UiTheme) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(400.0);
                self.local_card_list(ui);
            });
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("memory-card-local-details")
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - 6.0).max(260.0))
                    .show(ui, |ui| {
                        self.local_details(ui, ctx, theme);
                    });
            });
        });
    }

    fn local_card_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Cards");
        if ui.button("Open card…").clicked() {
            self.start_browse(BrowsePurpose::OpenLocal);
        }
        let rows: Vec<(usize, String, bool)> = self
            .cards
            .iter()
            .enumerate()
            .map(|(index, row)| (index, row.label(), self.selected_card == Some(index)))
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("memory-card-local-list")
            .max_height((ui.available_height() - 50.0).max(220.0))
            .show(ui, |ui| {
                for (index, label, selected) in rows {
                    if ui.selectable_label(selected, label).clicked() {
                        self.select_local(index);
                    }
                }
                if self.cards.is_empty() {
                    ui.weak("No .srm, .mcd, .mcr or .gme cards found in the local directory.");
                }
            });
    }

    fn local_details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &UiTheme) {
        let loaded = self.loaded.take();
        let mut action = None;

        if let Some(card) = &loaded {
            ui.heading(
                card.path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "memory card".to_string()),
            );
            ui.weak(card.path.display().to_string());
            ui.label(format!(
                "{} · {} save(s) · {} free slot(s)",
                card.format.label(),
                card.summary.saves.len(),
                card.summary.free_slots
            ));
            for warning in card.warnings.iter().take(4) {
                ui.colored_label(theme.warning, warning);
            }
            ui.separator();

            let mut new_save = None;
            for (index, save) in card.summary.saves.iter().enumerate() {
                let label = format!(
                    "#{}  {}  ·  {}  ·  {} block(s){}",
                    save.slot + 1,
                    save.game_title,
                    save.save_name,
                    save.blocks,
                    if save.deleted { "  (deleted)" } else { "" }
                );
                if ui
                    .selectable_label(
                        self.selected_save == Some(index),
                        egui::RichText::new(label).monospace(),
                    )
                    .clicked()
                {
                    new_save = Some(index);
                }
            }
            if card.summary.saves.is_empty() {
                ui.weak("No saves on this card.");
            }
            if let Some(index) = new_save {
                self.selected_save = Some(index);
                if let Some(save) = card.summary.saves.get(index) {
                    self.icon_name = default_icon_name(save);
                }
            }

            let selected_save = self
                .selected_save
                .and_then(|index| card.summary.saves.get(index));
            if let Some(save) = selected_save {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    self.update_icon_texture(ctx, card, self.selected_save);
                    if let Some(texture) = &self.icon_texture {
                        ui.image((texture.id(), egui::vec2(64.0, 64.0)));
                    }
                    ui.vertical(|ui| {
                        ui.label(format!(
                            "{} · {} · {} block(s) · {}",
                            save.save_name,
                            save.region,
                            save.blocks,
                            format_size(save.size_bytes)
                        ));
                        if save.deleted {
                            ui.colored_label(theme.warning, "This save is marked deleted.");
                        }
                    });
                });
            }

            ui.separator();
            let has_save = selected_save.is_some();
            let is_raw = card.format == CardFormat::Raw;

            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(is_raw, egui::Button::new("Import save…"))
                    .clicked()
                {
                    action = Some(LocalAction::ImportSourceBrowse);
                }
                if card.format == CardFormat::Gme && ui.button("Use as import source").clicked() {
                    action = Some(LocalAction::UseAsImportSource);
                }
                if ui
                    .add_enabled(has_save && is_raw, egui::Button::new("Delete save"))
                    .clicked()
                {
                    action = Some(LocalAction::Delete);
                }
                if ui
                    .add_enabled(has_save, egui::Button::new("Export icon"))
                    .clicked()
                {
                    action = Some(LocalAction::ExportIcon);
                }
            });

            if card.format == CardFormat::Gme {
                ui.separator();
                ui.label("Convert DexDrive card to raw");
                ui.horizontal(|ui| {
                    ui.label("Output");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.convert_name)
                            .desired_width(340.0)
                            .hint_text("name.srm or name.mcd"),
                    );
                    if ui.button("Convert .gme").clicked() {
                        action = Some(LocalAction::Convert);
                    }
                });
                ui.weak(
                    "The output is written into the local card directory and is never overwritten.",
                );
            }

            if has_save {
                ui.separator();
                ui.collapsing("Export icon options", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("PNG name");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.icon_name).desired_width(340.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Scale");
                        ui.add(
                            egui::DragValue::new(&mut self.icon_scale)
                                .range(1..=16)
                                .speed(1),
                        );
                        ui.checkbox(&mut self.icon_circle, "Circle frame");
                        ui.label("Palette");
                        egui::ComboBox::from_id_salt("memory-card-icon-palette")
                            .selected_text(icon_palette_label(self.icon_palette))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.icon_palette, None, "Default");
                                for palette in [
                                    IconPalette::Red,
                                    IconPalette::Green,
                                    IconPalette::Blue,
                                    IconPalette::Yellow,
                                    IconPalette::Purple,
                                    IconPalette::Orange,
                                ] {
                                    ui.selectable_value(
                                        &mut self.icon_palette,
                                        Some(palette),
                                        format!("{palette:?}"),
                                    );
                                }
                            });
                    });
                    ui.weak("Existing PNG files are never overwritten.");
                });
            }
        } else {
            ui.heading("Memory Card Manager");
            ui.weak("Select a card on the left, or open a .srm/.mcd/.gme file.");
            ui.add_space(8.0);
            ui.label("TsundereStation memory cards live in:");
            ui.monospace(self.local_dir.display().to_string());
            ui.weak(
                "Cards must be raw .srm images named after the game to be loaded by the emulator.",
            );
        }

        self.loaded = loaded;
        if let Some(action) = action {
            self.handle_local_action(action);
        }
    }

    fn import_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &UiTheme) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(430.0);
                self.source_card_list(ui);
            });
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("memory-card-source-details")
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - 6.0).max(260.0))
                    .show(ui, |ui| {
                        self.source_details(ui, ctx, theme);
                    });
            });
        });
    }

    fn source_card_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Source cards");
        if ui.button("Open source card…").clicked() {
            self.start_browse(BrowsePurpose::ImportSource);
        }
        let rows: Vec<(usize, String, bool)> = self
            .source_cards
            .iter()
            .enumerate()
            .map(|(index, row)| (index, row.label(), self.source_selected == Some(index)))
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("memory-card-source-list")
            .max_height((ui.available_height() - 50.0).max(220.0))
            .show(ui, |ui| {
                for (index, label, selected) in rows {
                    if ui.selectable_label(selected, label).clicked() {
                        self.select_source(index);
                    }
                }
                if self.source_cards.is_empty() {
                    ui.weak("No cards found in the source directory.");
                }
            });
    }

    fn source_details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &UiTheme) {
        let source = self.source_loaded.take();
        let mut action = None;

        if let Some(card) = &source {
            ui.heading(
                card.path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "source card".to_string()),
            );
            ui.weak(card.path.display().to_string());
            ui.label(format!(
                "{} · {} save(s) · {} free slot(s)",
                card.format.label(),
                card.summary.saves.len(),
                card.summary.free_slots
            ));
            if card.format == CardFormat::Gme {
                ui.colored_label(
                    theme.warning,
                    "DexDrive .gme is read-only here; copy or import converts it to a raw card.",
                );
            }
            for warning in card.warnings.iter().take(4) {
                ui.colored_label(theme.warning, warning);
            }
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.label("Copy to TsundereStation as");
                ui.add(
                    egui::TextEdit::singleline(&mut self.copy_name)
                        .desired_width(340.0)
                        .hint_text("Game (USA).srm"),
                );
                if ui
                    .button(if card.format == CardFormat::Gme {
                        "Convert & copy"
                    } else {
                        "Copy card"
                    })
                    .clicked()
                {
                    action = Some(SourceAction::CopyCard);
                }
            });
            if is_multi_disc(&self.copy_name) {
                ui.colored_label(
                    theme.warning,
                    "This looks like a multi-disc title. TsundereStation needs a card per game filename.",
                );
            }
            ui.separator();

            let mut new_save = None;
            for (index, save) in card.summary.saves.iter().enumerate() {
                let label = format!(
                    "#{}  {}  ·  {}  ·  {} block(s){}",
                    save.slot + 1,
                    save.game_title,
                    save.save_name,
                    save.blocks,
                    if save.deleted { "  (deleted)" } else { "" }
                );
                if ui
                    .selectable_label(
                        self.source_save_selected == Some(index),
                        egui::RichText::new(label).monospace(),
                    )
                    .clicked()
                {
                    new_save = Some(index);
                }
            }
            if card.summary.saves.is_empty() {
                ui.weak("No saves on this source card.");
            }
            if let Some(index) = new_save {
                self.source_save_selected = Some(index);
            }

            let selected_save = self
                .source_save_selected
                .and_then(|index| card.summary.saves.get(index));
            if let Some(save) = selected_save {
                ui.add_space(6.0);
                ui.label(format!(
                    "Selected save: {} · {} · {} block(s)",
                    save.save_name, save.region, save.blocks
                ));
                self.update_icon_texture(ctx, card, self.source_save_selected);
                if let Some(texture) = &self.icon_texture {
                    ui.image((texture.id(), egui::vec2(64.0, 64.0)));
                }
            }

            ui.separator();
            let target_name = self
                .selected_card
                .and_then(|index| self.cards.get(index))
                .map(CardRow::name)
                .unwrap_or_else(|| "no local card selected".to_string());
            let target_is_raw = self
                .loaded
                .as_ref()
                .is_some_and(|target| target.format == CardFormat::Raw);
            ui.label(format!("Import target: {target_name}"));
            if ui
                .add_enabled(
                    selected_save.is_some() && self.selected_card.is_some() && target_is_raw,
                    egui::Button::new("Import selected save"),
                )
                .clicked()
            {
                action = Some(SourceAction::ImportSave);
            }
            if target_is_raw {
                ui.weak("Change the target on the Local cards tab.");
            } else {
                ui.weak("Select a raw .srm/.mcd target card on the Local cards tab first.");
            }
        } else {
            ui.heading("Import & copy");
            ui.weak("Select a source card or use “Open source card…”.");
            ui.add_space(8.0);
            ui.label("DuckStation uses .mcd cards; RetroArch and TsundereStation use .srm.");
            ui.weak(
                "Raw card layouts are identical, so copying only changes the filename convention.",
            );
        }

        self.source_loaded = source;
        if let Some(action) = action {
            self.handle_source_action(action);
        }
    }

    fn select_local(&mut self, index: usize) {
        let Some(path) = self.cards.get(index).map(|row| row.path.clone()) else {
            return;
        };
        match load_card(&path) {
            Ok(card) => {
                let format = card.format;
                let first_save = card.summary.saves.first();
                self.selected_card = Some(index);
                self.selected_save = if first_save.is_some() { Some(0) } else { None };
                self.convert_name = if format == CardFormat::Gme {
                    format!(
                        "{}.srm",
                        path.file_stem()
                            .map(|stem| stem.to_string_lossy().to_string())
                            .unwrap_or_else(|| "memory-card".to_string())
                    )
                } else {
                    String::new()
                };
                self.icon_name = first_save.map(default_icon_name).unwrap_or_default();
                self.loaded = Some(card);
                self.error = None;
            }
            Err(error) => {
                self.loaded = None;
                self.selected_card = Some(index);
                self.selected_save = None;
                self.set_error(error.to_string());
            }
        }
    }

    fn select_source(&mut self, index: usize) {
        let Some(path) = self.source_cards.get(index).map(|row| row.path.clone()) else {
            return;
        };
        match load_card(&path) {
            Ok(card) => {
                self.source_selected = Some(index);
                self.source_save_selected = if card.summary.saves.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.copy_name = local_copy_name(&path);
                self.source_loaded = Some(card);
                self.error = None;
            }
            Err(error) => {
                self.source_loaded = None;
                self.source_selected = Some(index);
                self.set_error(error.to_string());
            }
        }
    }

    fn refresh_local(&mut self) {
        let selected_path = self
            .selected_card
            .and_then(|index| self.cards.get(index))
            .map(|row| row.path.clone());
        match scan_cards(&self.local_dir) {
            Ok(paths) => {
                self.cards = paths.into_iter().map(CardRow::from_path).collect();
                self.error = None;
            }
            Err(error) => {
                self.cards.clear();
                self.set_error(error.to_string());
            }
        }
        self.selected_card = None;
        self.loaded = None;
        self.selected_save = None;
        self.convert_name.clear();
        self.icon_name.clear();

        let index = selected_path
            .and_then(|path| self.cards.iter().position(|row| row.path == path))
            .or_else(|| (!self.cards.is_empty()).then_some(0));
        if let Some(index) = index {
            self.select_local(index);
        }
    }

    fn refresh_source(&mut self) {
        if self.source_dir.as_os_str().is_empty() {
            self.source_cards.clear();
            self.source_loaded = None;
            self.source_selected = None;
            return;
        }
        let selected_path = self
            .source_selected
            .and_then(|index| self.source_cards.get(index))
            .map(|row| row.path.clone());
        match scan_cards(&self.source_dir) {
            Ok(paths) => {
                self.source_cards = paths.into_iter().map(CardRow::from_path).collect();
                self.error = None;
            }
            Err(error) => {
                self.source_cards.clear();
                self.set_error(error.to_string());
            }
        }
        self.source_selected = None;
        self.source_loaded = None;
        self.source_save_selected = None;
        self.copy_name.clear();

        let index = selected_path
            .and_then(|path| self.source_cards.iter().position(|row| row.path == path))
            .or_else(|| (!self.source_cards.is_empty()).then_some(0));
        if let Some(index) = index {
            self.select_source(index);
        }
    }

    fn open_local_path(&mut self, path: PathBuf) {
        if !self.cards.iter().any(|row| row.path == path) {
            self.cards.push(CardRow::from_path(path.clone()));
        }
        if let Some(index) = self.cards.iter().position(|row| row.path == path) {
            self.tab = Tab::Local;
            self.select_local(index);
        }
    }

    fn open_source_path(&mut self, path: PathBuf) {
        if !self.source_cards.iter().any(|row| row.path == path) {
            self.source_cards.push(CardRow::from_path(path.clone()));
        }
        if let Some(index) = self.source_cards.iter().position(|row| row.path == path) {
            self.tab = Tab::Import;
            self.select_source(index);
        }
    }

    fn start_browse(&mut self, purpose: BrowsePurpose) {
        let start = match purpose {
            BrowsePurpose::OpenLocal => self.local_dir.clone(),
            BrowsePurpose::ImportSource => {
                if self.source_dir.as_os_str().is_empty() {
                    self.local_dir.clone()
                } else {
                    self.source_dir.clone()
                }
            }
        };
        self.file_browser = Some(FileBrowser::new(
            BrowseKind::MemoryCard,
            &start,
            vec![
                "srm".to_string(),
                "mcd".to_string(),
                "mcr".to_string(),
                "gme".to_string(),
            ],
        ));
        self.browse_purpose = Some(purpose);
    }

    fn poll_file_browser(&mut self, ctx: &egui::Context, theme: &UiTheme) {
        let Some(mut browser) = self.file_browser.take() else {
            return;
        };
        match browser.show(ctx, theme) {
            BrowseResult::None => {
                self.file_browser = Some(browser);
            }
            BrowseResult::Cancelled => {
                self.browse_purpose = None;
            }
            BrowseResult::Chosen(path) => {
                let purpose = self.browse_purpose.take();
                match purpose {
                    Some(BrowsePurpose::OpenLocal) => self.open_local_path(path),
                    Some(BrowsePurpose::ImportSource) => self.open_source_path(path),
                    None => {}
                }
            }
        }
    }

    fn handle_local_action(&mut self, action: LocalAction) {
        match action {
            LocalAction::Convert => self.convert_selected(),
            LocalAction::UseAsImportSource => {
                let Some(card) = self.loaded.as_ref() else {
                    return;
                };
                self.source_loaded = Some(card.clone());
                self.source_selected = None;
                self.source_save_selected = if card.summary.saves.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.copy_name = local_copy_name(&card.path);
                self.tab = Tab::Import;
            }
            LocalAction::ImportSourceBrowse => {
                self.start_browse(BrowsePurpose::ImportSource);
            }
            LocalAction::Delete => self.confirm_delete(),
            LocalAction::ExportIcon => self.export_icon(),
        }
    }

    fn handle_source_action(&mut self, action: SourceAction) {
        match action {
            SourceAction::CopyCard => self.start_copy_card(),
            SourceAction::ImportSave => self.start_import(),
        }
    }

    fn confirm_delete(&mut self) {
        let Some(card) = &self.loaded else {
            return;
        };
        if card.format != CardFormat::Raw {
            self.set_error("Convert the .gme card to a raw card before deleting saves.");
            return;
        }
        let Some(save) = self
            .selected_save
            .and_then(|index| card.summary.saves.get(index))
        else {
            return;
        };
        self.pending = Some((
            PendingAction::Delete {
                target: card.path.clone(),
                save_name: save.save_name.clone(),
            },
            format!(
                "Delete save “{}” from {}? A backup will be created.",
                save.save_name,
                card.path.display()
            ),
        ));
    }

    fn convert_selected(&mut self) {
        let Some(card) = self.loaded.take() else {
            self.set_error("no card selected");
            return;
        };
        let result: anyhow::Result<()> = (|| {
            if card.format != CardFormat::Gme {
                anyhow::bail!("only DexDrive .gme cards can be converted");
            }
            let name = normalized_output_name(&self.convert_name, "srm");
            if !has_card_extension(&name) {
                anyhow::bail!("output must end in .srm or .mcd");
            }
            let destination = self.local_dir.join(name);
            write_new_file_atomically(&destination, &card.raw)?;
            let loaded = load_card(&destination)?;
            if loaded.raw.len() != card.raw.len() {
                anyhow::bail!("converted card failed validation");
            }
            self.refresh_local();
            self.open_local_path(destination.clone());
            self.status = Some((
                format!(
                    "Converted {} to {}",
                    card.path.display(),
                    destination.display()
                ),
                MessageKind::Info,
            ));
            Ok(())
        })();
        match result {
            Ok(()) => {
                if self.loaded.is_none() {
                    self.loaded = Some(card);
                }
            }
            Err(error) => {
                self.loaded = Some(card);
                self.set_error(error.to_string());
            }
        }
    }

    fn export_icon(&mut self) {
        let result: anyhow::Result<()> = (|| {
            let card = self
                .loaded
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no card selected"))?;
            let save = self
                .selected_save
                .and_then(|index| card.summary.saves.get(index))
                .ok_or_else(|| anyhow::anyhow!("no save selected"))?;
            let mut icon = decode_icon(&card.raw, &save.save_name)?;
            if let Some(palette) = self.icon_palette {
                icon = remap_icon_palette(&icon, palette);
            }
            let png = encode_icon_png(&icon, self.icon_scale.clamp(1, 16), self.icon_circle)?;
            let name = normalized_output_name(&self.icon_name, "png");
            if !name.to_ascii_lowercase().ends_with(".png") {
                anyhow::bail!("icon output must end in .png");
            }
            let path = Path::new(&name);
            let destination = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.local_dir.join(path)
            };
            write_new_file_atomically(&destination, &png)?;
            self.status = Some((
                format!(
                    "Exported icon for {} to {}",
                    save.save_name,
                    destination.display()
                ),
                MessageKind::Info,
            ));
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn start_copy_card(&mut self) {
        let result: anyhow::Result<()> = (|| {
            let source = self
                .source_loaded
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no source card selected"))?;
            let name = normalized_output_name(&self.copy_name, "srm");
            if !name.to_ascii_lowercase().ends_with(".srm")
                && !name.to_ascii_lowercase().ends_with(".mcd")
            {
                anyhow::bail!("destination must end in .srm or .mcd");
            }
            let destination = self.local_dir.join(name);
            let message = format!(
                "Copy {} to {}?{}",
                source.path.display(),
                destination.display(),
                if destination.exists() {
                    "\nThe existing local card will be backed up first."
                } else {
                    ""
                }
            );
            if destination.exists() {
                self.pending = Some((
                    PendingAction::Copy {
                        source: source.path.clone(),
                        destination,
                    },
                    message,
                ));
            } else {
                self.execute_copy(source.path.clone(), destination);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn start_import(&mut self) {
        let result: anyhow::Result<()> = (|| {
            let source = self
                .source_loaded
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no source card selected"))?;
            let target = self
                .loaded
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("select a target card on the Local cards tab"))?;
            if target.format != CardFormat::Raw {
                anyhow::bail!("the target must be a raw .srm/.mcd card");
            }
            if super::io::same_file(&source.path, &target.path) {
                anyhow::bail!("source and target are the same card");
            }
            let save = self
                .source_save_selected
                .and_then(|index| source.summary.saves.get(index))
                .ok_or_else(|| anyhow::anyhow!("select a save to import"))?;
            let overwrite = target
                .summary
                .saves
                .iter()
                .any(|existing| existing.save_name == save.save_name);
            let message = format!(
                "Import “{}” from {} into {}?{}",
                save.save_name,
                source.path.display(),
                target.path.display(),
                if overwrite {
                    "\nA save with this name already exists and will be replaced."
                } else {
                    ""
                }
            );
            if overwrite {
                self.pending = Some((
                    PendingAction::Import {
                        target: target.path.clone(),
                        source: source.path.clone(),
                        save_name: save.save_name.clone(),
                        overwrite: true,
                    },
                    message,
                ));
            } else {
                self.execute_import(
                    target.path.clone(),
                    source.path.clone(),
                    save.save_name.clone(),
                    false,
                );
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn execute_pending(&mut self, action: PendingAction) {
        match action {
            PendingAction::Delete { target, save_name } => {
                self.execute_delete(target, save_name);
            }
            PendingAction::Import {
                target,
                source,
                save_name,
                overwrite,
            } => self.execute_import(target, source, save_name, overwrite),
            PendingAction::Copy {
                source,
                destination,
            } => self.execute_copy(source, destination),
        }
    }

    fn execute_delete(&mut self, target: PathBuf, save_name: String) {
        let result: anyhow::Result<()> = (|| {
            let raw = std::fs::read(&target)?;
            let (new_raw, report) = super::delete::delete_save(&raw, &save_name)?;
            super::card::parse_raw(&new_raw)?;
            let backup = replace_with_backup(&target, &new_raw)?;
            self.refresh_local();
            self.open_local_path(target.clone());
            self.status = Some((
                format!(
                    "Deleted {} ({} block(s), slot(s) {}){}",
                    report.save_name,
                    report.blocks,
                    report
                        .slots
                        .iter()
                        .map(|slot| (slot + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    backup
                        .map(|path| format!("; backup {}", path.display()))
                        .unwrap_or_default()
                ),
                MessageKind::Info,
            ));
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn execute_import(
        &mut self,
        target: PathBuf,
        source: PathBuf,
        save_name: String,
        overwrite: bool,
    ) {
        let result: anyhow::Result<()> = (|| {
            let source_card = load_card(&source)?;
            let target_raw = std::fs::read(&target)?;
            let (new_raw, report) = if overwrite {
                super::import::import_save_overwrite(&target_raw, &source_card.raw, &save_name)?
            } else {
                super::import::import_save(&target_raw, &source_card.raw, &save_name)?
            };
            super::card::parse_raw(&new_raw)?;
            let backup = replace_with_backup(&target, &new_raw)?;
            self.refresh_local();
            self.open_local_path(target.clone());
            self.status = Some((
                format!(
                    "Imported {} ({} block(s), slot(s) {}){}",
                    report.save_name,
                    report.blocks,
                    report
                        .slots
                        .iter()
                        .map(|slot| (slot + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    backup
                        .map(|path| format!("; backup {}", path.display()))
                        .unwrap_or_default()
                ),
                MessageKind::Info,
            ));
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn execute_copy(&mut self, source: PathBuf, destination: PathBuf) {
        let result: anyhow::Result<()> = (|| {
            let backup = copy_card(&source, &destination)?;
            if destination.parent() == Some(self.local_dir.as_path()) {
                self.refresh_local();
                self.open_local_path(destination.clone());
            } else if destination.parent() == Some(self.source_dir.as_path()) {
                self.refresh_source();
                self.select_source_path(destination.clone());
            }
            self.status = Some((
                format!(
                    "Copied {} to {}{}",
                    source.display(),
                    destination.display(),
                    backup
                        .map(|path| format!("; backup {}", path.display()))
                        .unwrap_or_default()
                ),
                MessageKind::Info,
            ));
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error.to_string());
        }
    }

    fn select_source_path(&mut self, path: PathBuf) {
        if !self.source_cards.iter().any(|row| row.path == path) {
            self.source_cards.push(CardRow::from_path(path.clone()));
        }
        if let Some(index) = self.source_cards.iter().position(|row| row.path == path) {
            self.select_source(index);
        }
    }

    fn show_confirm(&mut self, ctx: &egui::Context) {
        let Some((action, message)) = self.pending.take() else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::new("memory-card-confirm")).show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.heading("Confirm memory-card change");
            ui.add_space(6.0);
            ui.label(&message);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                if ui.button("Continue").clicked() {
                    decision = Some(true);
                }
            });
        });
        match decision {
            Some(true) => self.execute_pending(action),
            Some(false) => {}
            None => self.pending = Some((action, message)),
        }
    }

    fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.error = Some(message.clone());
        self.status = Some((message, MessageKind::Error));
    }

    fn update_icon_texture(
        &mut self,
        ctx: &egui::Context,
        card: &LoadedCard,
        save_index: Option<usize>,
    ) {
        let selected = save_index.and_then(|index| card.summary.saves.get(index));
        let key = selected.map(|save| format!("{}:{}", card.path.display(), save.save_name));
        if self.icon_texture_key == key {
            return;
        }
        self.icon_texture = None;
        self.icon_texture_key = key.clone();
        let Some(save) = selected else {
            return;
        };
        if let Ok(icon) = decode_icon(&card.raw, &save.save_name) {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([icon.width, icon.height], &icon.rgba);
            self.icon_texture = Some(ctx.load_texture(
                format!(
                    "memory-card-icon-{}:{}",
                    card.path.display(),
                    save.save_name
                ),
                image,
                egui::TextureOptions::NEAREST,
            ));
        }
    }
}

fn default_icon_name(save: &SlotInfo) -> String {
    format!(
        "{}-icon.png",
        save.save_name
            .chars()
            .map(|ch| if matches!(ch, '/' | '\\' | ':') {
                '_'
            } else {
                ch
            })
            .collect::<String>()
    )
}

fn icon_palette_label(palette: Option<IconPalette>) -> String {
    palette
        .map(|palette| format!("{palette:?}"))
        .unwrap_or_else(|| "Default".to_string())
}

fn has_card_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".srm") || lower.ends_with(".mcd")
}

fn normalized_output_name(name: &str, default_extension: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return format!("memory-card.{default_extension}");
    }
    if Path::new(trimmed).extension().is_some() {
        trimmed.to_string()
    } else {
        format!("{trimmed}.{default_extension}")
    }
}

fn format_size(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 1;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
