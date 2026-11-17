use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseKind {
    Game,
    Bios,
}

#[derive(Clone, Debug)]
pub enum BrowseResult {
    None,
    Cancelled,
    Chosen(PathBuf),
}

pub struct FileBrowser {
    pub kind: BrowseKind,
    pub open: bool,
    cwd: PathBuf,
    extensions: Vec<String>,
    entries: Vec<(PathBuf, String, bool)>,
    selected: Option<PathBuf>,
    error: Option<String>,
    needs_refresh: bool,
}

impl FileBrowser {
    pub fn new(kind: BrowseKind, start: &Path, extensions: Vec<String>) -> Self {
        let cwd = if start.is_dir() {
            start.to_path_buf()
        } else {
            start
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/"))
        };
        let mut browser = Self {
            kind,
            open: true,
            cwd,
            extensions: extensions.into_iter().map(|e| e.to_lowercase()).collect(),
            entries: Vec::new(),
            selected: None,
            error: None,
            needs_refresh: true,
        };
        browser.refresh();
        browser
    }

    fn refresh(&mut self) {
        self.needs_refresh = false;
        self.entries.clear();
        self.error = None;
        let read = match fs::read_dir(&self.cwd) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("Could not read {}: {e}", self.cwd.display()));
                return;
            }
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let is_dir = meta.is_dir();
            if !is_dir {
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !self.extensions.is_empty() && !self.extensions.iter().any(|e| e == &ext) {
                    continue;
                }
                if self.kind == BrowseKind::Bios && ext != "bin" && ext != "rom" && ext != "img" {
                    continue;
                }
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            self.entries.push((path, name, is_dir));
        }
        self.entries.sort_by(|a, b| match (a.2, b.2) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) -> BrowseResult {
        if !self.open {
            return BrowseResult::Cancelled;
        }
        if self.needs_refresh {
            self.refresh();
        }

        let mut result = BrowseResult::None;
        let mut open = self.open;
        let mut navigate_to = None;

        egui::Window::new(match self.kind {
            BrowseKind::Game => "Choose a game",
            BrowseKind::Bios => "Choose a BIOS image",
        })
        .open(&mut open)
        .resizable(true)
        .default_width(760.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Up").clicked() {
                    if let Some(parent) = self.cwd.parent() {
                        navigate_to = Some(parent.to_path_buf());
                    }
                }
                if ui.button("Home").clicked() {
                    navigate_to = Some(crate::config::home_dir());
                }
                if ui.button("Refresh").clicked() {
                    self.needs_refresh = true;
                }
            });
            ui.add(
                egui::Label::new(egui::RichText::new(self.cwd.display().to_string()).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            );
            ui.separator();

            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(ui.available_height() - 48.0)
                .show(ui, |ui| {
                    egui::Grid::new("file-browser-grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for (path, name, is_dir) in &self.entries {
                                let label = if *is_dir {
                                    format!("[dir] {name}")
                                } else {
                                    format!("[file] {name}")
                                };
                                let selected = self.selected.as_ref() == Some(path);
                                if ui.selectable_label(selected, label).clicked() {
                                    if *is_dir {
                                        navigate_to = Some(path.clone());
                                    } else {
                                        self.selected = Some(path.clone());
                                    }
                                }
                                if *is_dir {
                                    ui.label("");
                                } else {
                                    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                                    ui.weak(format_size(size));
                                }
                                ui.end_row();
                            }
                        });
                });

            ui.separator();
            ui.horizontal(|ui| {
                let can_choose = self.selected.is_some();
                if ui
                    .add_enabled(can_choose, egui::Button::new("Choose"))
                    .clicked()
                {
                    if let Some(path) = self.selected.clone() {
                        result = BrowseResult::Chosen(path);
                    }
                }
                if ui.button("Cancel").clicked() {
                    result = BrowseResult::Cancelled;
                }
                if let Some(path) = &self.selected {
                    ui.weak(path.display().to_string());
                }
            });
        });

        if let Some(dir) = navigate_to {
            self.cwd = dir;
            self.selected = None;
            self.needs_refresh = true;
        }

        if !open {
            result = BrowseResult::Cancelled;
        }
        if !matches!(result, BrowseResult::None) {
            self.open = false;
        }
        result
    }
}

fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}
