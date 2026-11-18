use crate::sdl_input::NavAction;
use crate::tv_ui::{controller_hint, tv_row};
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
    focus: usize,
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
            focus: 0,
            error: None,
            needs_refresh: true,
        };
        browser.refresh();
        browser.focus = if browser.entries.len() > 1 { 1 } else { 0 };
        browser.update_selected_from_focus();
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
        if let Some(parent) = self.cwd.parent() {
            self.entries
                .insert(0, (parent.to_path_buf(), "..".to_string(), true));
        }
        if self.entries.is_empty() {
            self.focus = 0;
        } else if self.focus >= self.entries.len() {
            self.focus = self.entries.len() - 1;
        }
        self.update_selected_from_focus();
    }

    /// Controller/keyboard navigation. Returns Some when the browser should
    /// close or a file has been chosen.
    pub fn navigate(&mut self, action: NavAction) -> Option<BrowseResult> {
        if self.needs_refresh {
            self.refresh();
        }
        if self.entries.is_empty() {
            return match action {
                NavAction::Cancel => {
                    self.open = false;
                    Some(BrowseResult::Cancelled)
                }
                _ => None,
            };
        }
        match action {
            NavAction::Up => {
                self.focus = (self.focus + self.entries.len() - 1) % self.entries.len();
                self.update_selected_from_focus();
            }
            NavAction::Down => {
                self.focus = (self.focus + 1) % self.entries.len();
                self.update_selected_from_focus();
            }
            NavAction::Left | NavAction::Right => {}
            NavAction::Confirm => {
                let (path, _name, is_dir) = self.entries[self.focus].clone();
                if is_dir {
                    self.cwd = path;
                    self.selected = None;
                    self.focus = 1;
                    self.needs_refresh = true;
                } else {
                    self.selected = Some(path.clone());
                    self.open = false;
                    return Some(BrowseResult::Chosen(path));
                }
            }
            NavAction::Cancel => {
                self.open = false;
                return Some(BrowseResult::Cancelled);
            }
        }
        None
    }

    fn update_selected_from_focus(&mut self) {
        self.selected = self
            .entries
            .get(self.focus)
            .and_then(|(path, _name, is_dir)| if *is_dir { None } else { Some(path.clone()) });
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
        .default_width(1100.0)
        .default_height(700.0)
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
                .max_height((ui.available_height() - 140.0).max(120.0))
                .show(ui, |ui| {
                    for (index, (path, name, is_dir)) in self.entries.iter().enumerate() {
                        let label = if name == ".." {
                            ".. (parent directory)".to_string()
                        } else if *is_dir {
                            format!("[dir] {name}")
                        } else {
                            let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                            format!("[file] {name}  —  {}", format_size(size))
                        };
                        let focused = self.focus == index;
                        let response = tv_row(ui, focused, &label);
                        if response.clicked() {
                            self.focus = index;
                            if *is_dir {
                                navigate_to = Some(path.clone());
                            } else {
                                self.selected = Some(path.clone());
                            }
                        }
                        if focused {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
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
            controller_hint(
                ui,
                "Cross: open/choose  ·  Circle: close  ·  .. row: parent",
            );
        });

        if let Some(dir) = navigate_to {
            self.cwd = dir;
            self.selected = None;
            self.focus = 1;
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
