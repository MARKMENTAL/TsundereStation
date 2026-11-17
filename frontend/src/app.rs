use crate::config::Config;
use crate::emu::{Command, WorkerEvent, WorkerHandle};
use crate::filebrowser::{BrowseKind, BrowseResult, FileBrowser};
use crate::libretro::host::{HostState, MessageKind};
use crate::libretro::loader::{state_path, CoreInfo};
use crate::sdl_input::{NavAction, SdlState};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Launcher,
    Game,
}

struct Toast {
    text: String,
    kind: MessageKind,
    until: Instant,
}

pub struct App {
    config: Config,
    host: Arc<HostState>,
    worker: WorkerHandle,
    core_info: Option<CoreInfo>,
    sdl: SdlState,

    screen: Screen,
    game_path: Option<String>,
    game_title: String,
    loading_game: bool,
    last_error: Option<String>,

    paused: bool,
    paused_by_overlay: bool,
    overlay_open: bool,
    menu_selection: usize,
    settings_open: bool,
    settings_category: String,
    settings_search: String,
    controller_open: bool,
    quick_slot: u32,

    texture: Option<egui::TextureHandle>,
    texture_seq: u64,
    texture_size: [usize; 2],

    toasts: Vec<Toast>,
    file_browser: Option<FileBrowser>,
    bios_files: Vec<(String, u64)>,

    port_device: Option<u32>,
    fullscreen: bool,
    auto_start: bool,
    auto_boot_bios: bool,
    did_auto_start: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mut config: Config,
        host: Arc<HostState>,
        core_path: PathBuf,
        auto_start: bool,
        auto_boot_bios: bool,
    ) -> anyhow::Result<Self> {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        host.video.set_context(cc.egui_ctx.clone());

        let sdl = SdlState::new(host.audio.clone(), &config.controller_guid)?;
        if !sdl.selected_guid().is_empty() && sdl.selected_guid() != config.controller_guid {
            config.controller_guid = sdl.selected_guid().to_string();
            let _ = config.save();
        }

        let fullscreen = config.fullscreen;
        let worker = WorkerHandle::spawn(
            core_path,
            host.clone(),
            config.core_options.clone(),
            config.state_dir.clone(),
        );

        let port_device = config_port_device(&config);
        let mut app = Self {
            config,
            host,
            worker,
            core_info: None,
            sdl,
            screen: Screen::Launcher,
            game_path: None,
            game_title: String::new(),
            loading_game: false,
            last_error: None,
            paused: false,
            paused_by_overlay: false,
            overlay_open: false,
            menu_selection: 0,
            settings_open: false,
            settings_category: String::new(),
            settings_search: String::new(),
            controller_open: false,
            quick_slot: 0,
            texture: None,
            texture_seq: 0,
            texture_size: [0, 0],
            toasts: Vec::new(),
            file_browser: None,
            bios_files: Vec::new(),
            port_device,
            fullscreen,
            auto_start,
            auto_boot_bios,
            did_auto_start: false,
        };
        app.refresh_bios_files();
        if fullscreen {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        Ok(app)
    }

    fn process_worker_events(&mut self) {
        let events = self.worker.try_recv_events();
        for event in events {
            match event {
                WorkerEvent::Ready(info) => {
                    self.resolve_port_device(&info);
                    self.core_info = Some(info);
                    self.last_error = None;
                    if !self.did_auto_start {
                        self.did_auto_start = true;
                        if self.auto_boot_bios {
                            self.start_bios();
                        } else if self.auto_start {
                            self.start_game();
                        }
                    }
                }
                WorkerEvent::Booted { title, path } => {
                    self.game_title = title;
                    if path.is_empty() {
                        self.game_path = None;
                    } else {
                        self.game_path = Some(path.clone());
                        self.config.last_game = path.clone();
                        self.config.add_recent(Path::new(&path));
                        let _ = self.config.save();
                    }
                    self.loading_game = false;
                    self.last_error = None;
                    self.screen = Screen::Game;
                    self.overlay_open = false;
                    self.paused = false;
                    self.texture = None;
                    self.texture_seq = 0;
                    self.texture_size = [0, 0];
                    self.host
                        .fast_forward
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    self.worker.send(Command::Pause(false));
                }
                WorkerEvent::Unloaded => {
                    self.screen = Screen::Launcher;
                    self.game_path = None;
                    self.loading_game = false;
                    self.overlay_open = false;
                    self.paused = false;
                    self.paused_by_overlay = false;
                    self.settings_open = false;
                    self.controller_open = false;
                    self.host
                        .fast_forward
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    self.texture = None;
                    self.texture_seq = 0;
                    self.texture_size = [0, 0];
                }
                WorkerEvent::StateSaved(slot) => {
                    self.push_toast(format!("Saved state slot {slot}"), MessageKind::Info, 2.0);
                }
                WorkerEvent::StateLoaded(slot) => {
                    self.push_toast(format!("Loaded state slot {slot}"), MessageKind::Info, 2.0);
                }
                WorkerEvent::Error(message) => {
                    self.loading_game = false;
                    self.last_error = Some(message.clone());
                    self.push_toast(message, MessageKind::Error, 6.0);
                }
                WorkerEvent::Stopped => {
                    self.loading_game = false;
                }
            }
        }
    }

    fn resolve_port_device(&mut self, info: &CoreInfo) {
        let Some(port0) = info.ports.first() else {
            return;
        };
        let valid = |id: u32| port0.types.iter().any(|(_, i)| *i == id);
        let chosen = self
            .config
            .port1_device
            .filter(|id| valid(*id))
            .or_else(|| {
                port0
                    .types
                    .iter()
                    .find(|(name, _)| name.contains("DualShock") || name.contains("Analog"))
                    .map(|(_, id)| *id)
            })
            .or_else(|| port0.types.first().map(|(_, id)| *id));
        if let Some(id) = chosen {
            self.port_device = Some(id);
            self.config.port1_device = Some(id);
        }
    }

    fn refresh_bios_files(&mut self) {
        self.bios_files.clear();
        let Ok(read) = std::fs::read_dir(&self.config.bios_dir) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let is_bios = path
                .extension()
                .map(|e| {
                    let e = e.to_string_lossy();
                    e.eq_ignore_ascii_case("bin")
                        || e.eq_ignore_ascii_case("rom")
                        || e.eq_ignore_ascii_case("img")
                })
                .unwrap_or(false);
            if !is_bios {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let name = entry.file_name().to_string_lossy().to_string();
            self.bios_files.push((name, size));
        }
        self.bios_files.sort_by(|a, b| a.0.cmp(&b.0));
    }

    fn push_toast(&mut self, text: String, kind: MessageKind, seconds: f32) {
        let until = Instant::now() + Duration::from_secs_f32(seconds.max(0.5));
        self.toasts.push(Toast { text, kind, until });
        if self.toasts.len() > 8 {
            self.toasts.remove(0);
        }
    }

    fn process_host_messages(&mut self) {
        let now = Instant::now();
        for message in self.host.take_messages() {
            let until = message.created + message.ttl;
            if until > now {
                self.toasts.push(Toast {
                    text: message.text,
                    kind: message.kind,
                    until,
                });
            }
        }
        self.toasts.retain(|t| t.until > now);
        if self.toasts.len() > 8 {
            let excess = self.toasts.len() - 8;
            self.toasts.drain(0..excess);
        }
    }

    fn poll_sdl(&mut self, ctx: &egui::Context) {
        let menu = self.sdl.poll(&self.host, &self.config);
        if !self.sdl.selected_guid().is_empty()
            && self.sdl.selected_guid() != self.config.controller_guid
        {
            self.config.controller_guid = self.sdl.selected_guid().to_string();
            let _ = self.config.save();
        }
        if menu && self.screen == Screen::Game {
            if self.settings_open {
                self.settings_open = false;
            } else if self.controller_open {
                self.controller_open = false;
            } else {
                self.toggle_overlay();
            }
        }

        if self.overlay_open && !self.paused && !self.settings_open && !self.controller_open {
            // When the game keeps running behind the menu, do not let menu
            // navigation leak into the emulated pad. While paused we leave the
            // snapshot live so the controller diagnostic window can show it.
            self.host.input.set(0, [0; 4]);
        }
        let nav_active = self.overlay_open && !self.settings_open && !self.controller_open;
        for nav in self.sdl.poll_menu_nav(nav_active) {
            self.handle_overlay_nav(nav, ctx);
        }
    }

    fn handle_overlay_nav(&mut self, nav: NavAction, ctx: &egui::Context) {
        match nav {
            NavAction::Up => {
                self.menu_selection = (self.menu_selection + 8) % 9;
            }
            NavAction::Down => {
                self.menu_selection = (self.menu_selection + 1) % 9;
            }
            NavAction::Left => {
                self.quick_slot = (self.quick_slot + 9) % 10;
            }
            NavAction::Right => {
                self.quick_slot = (self.quick_slot + 1) % 10;
            }
            NavAction::Cancel => self.close_overlay(),
            NavAction::Confirm => self.activate_overlay_action(ctx),
        }
    }

    fn activate_overlay_action(&mut self, ctx: &egui::Context) {
        match self.menu_selection {
            0 => self.close_overlay(),
            1 => {
                self.worker.send(Command::Reset);
                self.close_overlay();
            }
            2 => self.worker.send(Command::SaveState(self.quick_slot)),
            3 => self.worker.send(Command::LoadState(self.quick_slot)),
            4 => self.settings_open = true,
            5 => self.controller_open = true,
            6 => self.toggle_fullscreen(ctx),
            7 => self.quit_to_launcher(),
            8 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            _ => {}
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        if self.screen != Screen::Game {
            return;
        }
        // Game hotkeys are disabled while one of the frontend's text-entry
        // windows is open. Selectable labels can take focus too, so don't use
        // egui_wants_keyboard_input() here.
        let typing = self.settings_open || self.controller_open || self.file_browser.is_some();
        if !typing {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F1)) {
                self.toggle_overlay();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
                self.worker.send(Command::SaveState(self.quick_slot));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
                self.worker.send(Command::LoadState(self.quick_slot));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
                self.toggle_fullscreen(ctx);
            }
            if self.overlay_open && !self.settings_open && !self.controller_open {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.handle_overlay_nav(NavAction::Up, ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.handle_overlay_nav(NavAction::Down, ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                    self.handle_overlay_nav(NavAction::Left, ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                    self.handle_overlay_nav(NavAction::Right, ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.handle_overlay_nav(NavAction::Confirm, ctx);
                }
            }
        }
        let fast = !self.overlay_open
            && !self.settings_open
            && !self.controller_open
            && !typing
            && ctx.input(|i| i.key_down(egui::Key::Tab));
        self.host
            .fast_forward
            .store(fast, std::sync::atomic::Ordering::Relaxed);
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        self.config.fullscreen = self.fullscreen;
        let _ = self.config.save();
    }

    fn toggle_overlay(&mut self) {
        if self.overlay_open {
            self.close_overlay();
        } else {
            self.open_overlay();
        }
    }

    fn open_overlay(&mut self) {
        if self.overlay_open {
            return;
        }
        self.overlay_open = true;
        self.menu_selection = 0;
        self.paused_by_overlay = false;
        if self.config.input.pause_on_menu && !self.paused {
            self.paused = true;
            self.paused_by_overlay = true;
            self.worker.send(Command::Pause(true));
        }
    }

    fn close_overlay(&mut self) {
        if !self.overlay_open {
            return;
        }
        self.overlay_open = false;
        self.settings_open = false;
        if self.paused_by_overlay {
            self.paused = false;
            self.paused_by_overlay = false;
            self.worker.send(Command::Pause(false));
        }
    }

    fn start_bios(&mut self) {
        if self.core_info.is_none() {
            return;
        }
        self.loading_game = true;
        self.last_error = None;
        self.worker.send(Command::BootBios {
            port_device: self.port_device,
        });
    }

    fn start_game(&mut self) {
        let path = self.config.last_game.trim().to_string();
        if path.is_empty() {
            self.push_toast("Choose a game first".into(), MessageKind::Warning, 3.0);
            return;
        }
        if !Path::new(&path).exists() {
            self.push_toast(format!("Game not found: {path}"), MessageKind::Error, 5.0);
            return;
        }
        if self.core_info.is_none() {
            self.push_toast("Core is still starting".into(), MessageKind::Warning, 3.0);
            return;
        }
        self.loading_game = true;
        self.last_error = None;
        self.worker.send(Command::LoadGame {
            path,
            port_device: self.port_device,
        });
    }

    fn quit_to_launcher(&mut self) {
        self.worker.send(Command::Pause(false));
        self.worker.send(Command::UnloadGame);
        self.host
            .fast_forward
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.screen = Screen::Launcher;
        self.overlay_open = false;
        self.settings_open = false;
        self.controller_open = false;
        self.paused = false;
        self.paused_by_overlay = false;
        self.loading_game = false;
        self.texture = None;
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let Some((frame, seq)) = self.host.video.latest() else {
            return;
        };
        if seq == self.texture_seq && self.texture.is_some() {
            return;
        }
        let image =
            egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        if let Some(texture) = self.texture.as_mut() {
            if self.texture_size == [frame.width, frame.height] {
                texture.set(image, egui::TextureOptions::NEAREST);
                self.texture_seq = seq;
                return;
            }
        }
        self.texture = Some(ctx.load_texture(
            format!("game-frame-{seq}"),
            image,
            egui::TextureOptions::NEAREST,
        ));
        self.texture_seq = seq;
        self.texture_size = [frame.width, frame.height];
    }

    fn launcher(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading("SwanStation");
                    ui.label(
                        egui::RichText::new("PlayStation emulator")
                            .weak()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(info) = &self.core_info {
                            ui.weak(format!("{} {}", info.name, info.version));
                        } else {
                            ui.spinner();
                            ui.weak("Loading core…");
                        }
                    });
                });
                ui.add_space(6.0);

                if let Some(error) = &self.last_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                    ui.add_space(4.0);
                }

                ui.group(|ui| {
                    ui.heading("BIOS");
                    ui.label(
                        "BIOS is optional. If none is present, the built-in OpenBIOS is used. \
                         Files must be valid PS1 BIOS images (512 KiB or 4 MiB). When several \
                         images are present, the core picks one matching the disc region.",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Folder:");
                        ui.monospace(self.config.bios_dir.display().to_string());
                        if ui.button("Open").clicked() {
                            let _ = std::process::Command::new("xdg-open")
                                .arg(&self.config.bios_dir)
                                .spawn();
                        }
                        if ui.button("Add BIOS image…").clicked() {
                            self.file_browser = Some(FileBrowser::new(
                                BrowseKind::Bios,
                                &crate::config::home_dir(),
                                vec!["bin".into(), "rom".into(), "img".into()],
                            ));
                        }
                    });
                    if self.bios_files.is_empty() {
                        ui.weak("No BIOS images in the folder. OpenBIOS will be used.");
                    } else {
                        for (name, size) in self.bios_files.clone() {
                            let valid = matches!(size, 0x80000 | 0x400000 | 0x3E66F0);
                            ui.horizontal(|ui| {
                                ui.monospace(name);
                                let text = format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0));
                                if valid {
                                    ui.weak(text);
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 150, 60),
                                        format!("{text} — unexpected size"),
                                    );
                                }
                            });
                        }
                    }
                });

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.heading("Game");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.last_game)
                                .desired_width(560.0)
                                .hint_text("/path/to/game.cue"),
                        );
                        if ui.button("Browse…").clicked() {
                            let start = Path::new(&self.config.last_game)
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(crate::config::home_dir);
                            let extensions = self
                                .core_info
                                .as_ref()
                                .map(|i| {
                                    i.extensions
                                        .split('|')
                                        .map(|s| s.to_string())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_else(|| {
                                    vec![
                                        "cue".into(),
                                        "bin".into(),
                                        "chd".into(),
                                        "iso".into(),
                                        "pbp".into(),
                                        "m3u".into(),
                                    ]
                                });
                            self.file_browser =
                                Some(FileBrowser::new(BrowseKind::Game, &start, extensions));
                        }
                    });
                    ui.horizontal(|ui| {
                        let enabled = self.core_info.is_some() && !self.loading_game;
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("Play").min_size(egui::vec2(110.0, 30.0)),
                            )
                            .clicked()
                        {
                            self.start_game();
                        }
                        if ui
                            .add_enabled(
                                self.core_info.is_some() && !self.loading_game,
                                egui::Button::new("Boot BIOS").min_size(egui::vec2(100.0, 30.0)),
                            )
                            .clicked()
                        {
                            self.start_bios();
                        }
                        if self.loading_game {
                            ui.spinner();
                            ui.weak("Loading…");
                        }
                    });
                    if !self.config.recent_games.is_empty() {
                        ui.add_space(4.0);
                        ui.label("Recent games");
                        for path in self.config.recent_games.clone() {
                            if ui.selectable_label(false, &path).clicked() {
                                self.config.last_game = path;
                            }
                        }
                    }
                });

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.heading("Controller");
                    ui.horizontal(|ui| {
                        let controllers = self.sdl.controllers().to_vec();
                        let current = self
                            .sdl
                            .selected_name()
                            .unwrap_or("No controller detected")
                            .to_string();
                        egui::ComboBox::from_id_salt("launcher-controller")
                            .width(380.0)
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                if controllers.is_empty() {
                                    ui.weak("No SDL game controllers found");
                                }
                                for controller in &controllers {
                                    let selected = self.sdl.selected_guid() == controller.guid;
                                    if ui.selectable_label(selected, &controller.name).clicked() {
                                        self.sdl.select_guid(&controller.guid);
                                        self.config.controller_guid = controller.guid.clone();
                                        let _ = self.config.save();
                                    }
                                }
                            });
                        if ui.button("Rescan").clicked() {
                            self.sdl.refresh_controllers(None);
                        }
                        if ui.button("Controller settings…").clicked() {
                            self.controller_open = true;
                        }
                    });
                    ui.weak("Press the PS/Guide button in-game to open the menu.");
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Settings…").clicked() {
                        self.settings_open = true;
                    }
                    if ui.button("Toggle fullscreen").clicked() {
                        self.toggle_fullscreen(ui.ctx());
                    }
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
    }

    fn game(&mut self, ui: &mut egui::Ui) {
        self.update_texture(ui.ctx());
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::BLACK);
            if let Some(texture) = &self.texture {
                let [w, h] = [self.texture_size[0] as f32, self.texture_size[1] as f32];
                let scale = (rect.width() / w).min(rect.height() / h).max(0.01);
                let size = egui::vec2(w * scale, h * scale);
                let image_rect = egui::Rect::from_center_size(rect.center(), size);
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Starting…",
                    egui::FontId::proportional(24.0),
                    egui::Color32::GRAY,
                );
            }

            let mut info = format!("{:.2} Hz", self.host.fps());
            if self.paused {
                info.push_str(" · paused");
            }
            if self
                .host
                .fast_forward
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                info.push_str(" · >>");
            }
            ui.painter().text(
                rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                info,
                egui::FontId::monospace(13.0),
                egui::Color32::from_white_alpha(170),
            );
        });

        if self.overlay_open {
            self.overlay_window(ui.ctx());
        }
    }

    fn overlay_window(&mut self, ctx: &egui::Context) {
        let title = if self.game_title.is_empty() {
            "Menu".to_string()
        } else {
            self.game_title.clone()
        };
        let fps = self.host.fps();
        let state_identity = self
            .game_path
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new("bios"));
        let state_exists =
            state_path(&self.config.state_dir, state_identity, self.quick_slot).exists();
        let labels = [
            "Resume".to_string(),
            "Reset console".to_string(),
            format!("Save state (slot {})", self.quick_slot),
            format!("Load state (slot {})", self.quick_slot),
            "Settings…".to_string(),
            "Controller…".to_string(),
            "Toggle fullscreen (F11)".to_string(),
            "Quit to launcher".to_string(),
            "Quit SwanStation".to_string(),
        ];
        let mut clicked = None;

        egui::Window::new(title)
            .id(egui::Id::new("game-menu"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(400.0);
                ui.heading(if self.paused { "Paused" } else { "Menu" });
                ui.weak(format!("{fps:.2} Hz · slot {}", self.quick_slot));
                ui.horizontal(|ui| {
                    ui.label("Slot");
                    egui::ComboBox::from_id_salt("save-slot")
                        .selected_text(self.quick_slot.to_string())
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for slot in 0..=9 {
                                ui.selectable_value(&mut self.quick_slot, slot, slot.to_string());
                            }
                        });
                    ui.weak(if state_exists {
                        "state exists"
                    } else {
                        "empty"
                    });
                });
                ui.separator();
                for (index, label) in labels.iter().enumerate() {
                    if ui
                        .selectable_label(self.menu_selection == index, label)
                        .clicked()
                    {
                        self.menu_selection = index;
                        clicked = Some(index);
                    }
                }
                ui.separator();
                ui.weak("D-pad/stick: move - Cross: select - Circle/PS: close - Left/Right: slot");
            });

        if let Some(index) = clicked {
            self.menu_selection = index;
            self.activate_overlay_action(ctx);
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let Some(info) = self.core_info.as_ref() else {
            return;
        };
        let host = self.host.clone();
        let mut changes: Vec<(String, String)> = Vec::new();
        let mut resets: Vec<String> = Vec::new();
        let mut reset_all = false;
        let mut restart_core = false;
        let mut open = self.settings_open;

        let visible = host.options.read().unwrap().visible.clone();
        let overrides = host.options.read().unwrap().overrides.clone();
        let categories = info.categories.clone();
        let options = info.options.clone();

        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_width(900.0)
            .default_height(620.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_search)
                            .desired_width(260.0)
                            .hint_text("filter options"),
                    );
                    if ui.button("Reset all").clicked() {
                        reset_all = true;
                    }
                    if ui.button("Restart core").clicked() {
                        restart_core = true;
                    }
                    ui.weak("Renderer: Software");
                });
                ui.weak(
                    "Most options apply on the next frame. Use Restart core to re-read all options; \
                     options marked \"requires restart\" need it.",
                );
                ui.separator();

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(180.0);
                        if ui
                            .selectable_label(self.settings_category.is_empty(), "All options")
                            .clicked()
                        {
                            self.settings_category.clear();
                        }
                        if ui
                            .selectable_label(
                                self.settings_category == "uncategorized",
                                "Uncategorized",
                            )
                            .clicked()
                        {
                            self.settings_category = "uncategorized".to_string();
                        }
                        for category in &categories {
                            let mut response = ui.selectable_label(
                                self.settings_category == category.key,
                                if category.label.is_empty() {
                                    &category.key
                                } else {
                                    &category.label
                                },
                            );
                            if !category.info.is_empty() {
                                response = response.on_hover_text(&category.info);
                            }
                            if response.clicked() {
                                self.settings_category = category.key.clone();
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(650.0);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(520.0)
                            .show(ui, |ui| {
                                let search = self.settings_search.to_lowercase();
                                let mut shown = 0;
                                egui::Grid::new("settings-options-grid")
                                    .num_columns(2)
                                    .striped(true)
                                    .spacing([16.0, 8.0])
                                    .show(ui, |ui| {
                                        for option in &options {
                                            if option.key == "swanstation_GPU_Renderer" {
                                                continue;
                                            }
                                            if !visible.get(&option.key).copied().unwrap_or(true) {
                                                continue;
                                            }
                                            let category_matches = self
                                                .settings_category
                                                .is_empty()
                                                || (self.settings_category == "uncategorized"
                                                    && (option.category_key.is_empty()
                                                        || !categories.iter().any(|c| {
                                                            c.key == option.category_key
                                                        })))
                                                || option.category_key == self.settings_category;
                                            if !category_matches {
                                                continue;
                                            }
                                            if !search.is_empty()
                                                && !option.label.to_lowercase().contains(&search)
                                                && !option.info.to_lowercase().contains(&search)
                                                && !option.key.to_lowercase().contains(&search)
                                            {
                                                continue;
                                            }
                                            shown += 1;
                                            let current = host
                                                .option_value(&option.key)
                                                .unwrap_or_else(|| option.default.clone());
                                            let current_label = option
                                                .values
                                                .iter()
                                                .find(|(v, _)| v == &current)
                                                .map(|(_, l)| l.as_str())
                                                .unwrap_or(current.as_str());
                                            let restart = option.label.contains("Restart")
                                                || option.info.contains("Restart")
                                                || option.info.contains("restart");
                                            ui.vertical(|ui| {
                                                ui.set_min_width(300.0);
                                                ui.strong(&option.label);
                                                if restart {
                                                    ui.small(
                                                        egui::RichText::new("requires restart")
                                                            .color(egui::Color32::from_rgb(
                                                                220, 170, 60,
                                                            )),
                                                    );
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                let response = egui::ComboBox::from_id_salt(
                                                    format!("opt-{}", option.key),
                                                )
                                                .width(250.0)
                                                .selected_text(current_label)
                                                .show_ui(ui, |ui| {
                                                    for (value, label) in &option.values {
                                                        if ui
                                                            .selectable_label(
                                                                value == &current,
                                                                if label.is_empty() {
                                                                    value
                                                                } else {
                                                                    label
                                                                },
                                                            )
                                                            .clicked()
                                                        {
                                                            changes.push((
                                                                option.key.clone(),
                                                                value.clone(),
                                                            ));
                                                        }
                                                    }
                                                });
                                                if overrides.contains_key(&option.key)
                                                    && ui.small_button("reset").clicked()
                                                {
                                                    resets.push(option.key.clone());
                                                }
                                                if !option.info.is_empty() {
                                                    response.response.on_hover_text(&option.info);
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    });
                                if shown == 0 {
                                    ui.weak("No matching options.");
                                }
                            });
                    });
                });
            });
        self.settings_open = open;

        let options_changed = !changes.is_empty() || !resets.is_empty() || reset_all;
        for key in resets {
            self.host.reset_option(&key);
        }
        for (key, value) in changes {
            self.host.set_option(&key, value);
        }
        if reset_all {
            self.host.options.write().unwrap().reset_all();
        }
        if options_changed {
            self.config.core_options = self
                .host
                .options
                .read()
                .unwrap()
                .overrides
                .clone()
                .into_iter()
                .collect();
            let _ = self.config.save();
        }

        // Boot-time options (region, BIOS paths, fast boot, renderer, ...) are
        // read by retro_init/retro_load_game. When nothing is running, reinit
        // the core so the next boot uses the new values. While a game is
        // running, the user can use the Restart core button.
        let mut do_restart = restart_core;
        if options_changed && self.screen == Screen::Launcher && !self.loading_game {
            do_restart = true;
        }
        if do_restart {
            self.worker.send(Command::RestartCore);
            self.push_toast(
                if restart_core {
                    "Restarting core…"
                } else {
                    "Applying settings…"
                }
                .into(),
                MessageKind::Info,
                2.0,
            );
        }
    }

    fn controller_window(&mut self, ctx: &egui::Context) {
        if !self.controller_open {
            return;
        }
        let mut open = self.controller_open;
        let mut dirty = false;
        let mut device_change = None;
        let mut rescan = false;
        let info = self.core_info.clone();
        let host = self.host.clone();

        let sdl = &mut self.sdl;
        let config = &mut self.config;

        egui::Window::new("Controller")
            .open(&mut open)
            .resizable(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading("SDL game controller");
                let controllers = sdl.controllers().to_vec();
                let current = sdl
                    .selected_name()
                    .unwrap_or("No controller detected")
                    .to_string();
                egui::ComboBox::from_id_salt("controller-window-select")
                    .width(360.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if controllers.is_empty() {
                            ui.weak("No SDL game controllers found");
                        }
                        for controller in &controllers {
                            if ui
                                .selectable_label(
                                    sdl.selected_guid() == controller.guid,
                                    &controller.name,
                                )
                                .clicked()
                            {
                                sdl.select_guid(&controller.guid);
                                config.controller_guid = controller.guid.clone();
                                dirty = true;
                            }
                        }
                    });
                if ui.button("Rescan controllers").clicked() {
                    rescan = true;
                }
                ui.weak("PS / Guide opens the in-game menu. If Steam opens instead, disable Steam Input for this controller.");
                ui.separator();

                if let Some(info) = &info {
                    if let Some(port0) = info.ports.first() {
                        let current_id = config
                            .port1_device
                            .or_else(|| port0.types.first().map(|(_, id)| *id));
                        let current_name = current_id
                            .and_then(|id| {
                                port0
                                    .types
                                    .iter()
                                    .find(|(_, i)| *i == id)
                                    .map(|(n, _)| n.clone())
                            })
                            .unwrap_or_else(|| "Unknown".to_string());
                        egui::ComboBox::from_id_salt("device-type")
                            .width(300.0)
                            .selected_text(current_name)
                            .show_ui(ui, |ui| {
                                for (name, id) in &port0.types {
                                    if ui
                                        .selectable_label(
                                            Some(*id) == current_id,
                                            name,
                                        )
                                        .clicked()
                                    {
                                        config.port1_device = Some(*id);
                                        device_change = Some(*id);
                                        dirty = true;
                                    }
                                }
                            });
                    }
                }

                ui.add(
                    egui::Slider::new(&mut config.input.deadzone, 0.0..=0.6)
                        .text("Stick deadzone"),
                );
                if ui
                    .checkbox(
                        &mut config.input.pause_on_menu,
                        "Pause game while the menu is open",
                    )
                    .changed()
                {
                    dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Fallback menu combo:");
                    if ui
                        .selectable_label(
                            config.input.menu_button == "guide",
                            "None (PS/Guide only)",
                        )
                        .clicked()
                    {
                        config.input.menu_button = "guide".into();
                        dirty = true;
                    }
                    if ui
                        .selectable_label(
                            config.input.menu_button == "select_start",
                            "Select + Start",
                        )
                        .clicked()
                    {
                        config.input.menu_button = "select_start".into();
                        dirty = true;
                    }
                });
                ui.separator();
                ui.label("Live input");
                ui.monospace(format!(
                    "buttons: {:016b}   axes: {} {} {} {}",
                    host.input.buttons() & 0xffff,
                    host.input.axis(0, 0),
                    host.input.axis(0, 1),
                    host.input.axis(1, 0),
                    host.input.axis(1, 1),
                ));
                ui.monospace(format!("last button: {}", sdl.last_button()));
            });

        self.controller_open = open;
        if rescan {
            self.sdl.refresh_controllers(None);
            dirty = true;
        }
        if let Some(id) = device_change {
            self.port_device = Some(id);
            self.worker.send(Command::SetPortDevice(0, id));
        }
        if dirty {
            self.config.controller_guid = self.sdl.selected_guid().to_string();
            let _ = self.config.save();
        }
    }

    fn browser_window(&mut self, ctx: &egui::Context) {
        let mut result = BrowseResult::None;
        let kind = self.file_browser.as_ref().map(|b| b.kind);
        if let Some(browser) = self.file_browser.as_mut() {
            result = browser.show(ctx);
        }
        if matches!(result, BrowseResult::None) {
            return;
        }
        self.file_browser = None;
        if let (BrowseResult::Chosen(path), Some(kind)) = (result, kind) {
            match kind {
                BrowseKind::Game => {
                    self.config.last_game = path.to_string_lossy().to_string();
                    let _ = self.config.save();
                }
                BrowseKind::Bios => {
                    let name = path
                        .file_name()
                        .map(|s| s.to_os_string())
                        .unwrap_or_else(|| "bios.bin".into());
                    let dest = self.config.bios_dir.join(name);
                    let same_file =
                        std::fs::canonicalize(&path).ok() == std::fs::canonicalize(&dest).ok();
                    if same_file {
                        self.refresh_bios_files();
                        self.push_toast(
                            format!("BIOS already in {}", dest.display()),
                            MessageKind::Info,
                            4.0,
                        );
                        return;
                    }
                    match std::fs::copy(&path, &dest) {
                        Ok(_) => {
                            self.refresh_bios_files();
                            self.push_toast(
                                format!("Added BIOS {}", dest.display()),
                                MessageKind::Info,
                                4.0,
                            );
                        }
                        Err(e) => {
                            self.push_toast(
                                format!("Could not copy BIOS: {e}"),
                                MessageKind::Error,
                                6.0,
                            );
                        }
                    }
                }
            }
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0))
            .show(ctx, |ui| {
                ui.set_max_width(430.0);
                for toast in &self.toasts {
                    let color = match toast.kind {
                        MessageKind::Info => egui::Color32::from_rgb(40, 90, 140),
                        MessageKind::Warning => egui::Color32::from_rgb(140, 100, 30),
                        MessageKind::Error => egui::Color32::from_rgb(150, 45, 45),
                        MessageKind::Debug => egui::Color32::from_rgb(70, 70, 70),
                    };
                    egui::Frame::NONE
                        .fill(color)
                        .inner_margin(egui::Margin::same(8))
                        .corner_radius(egui::CornerRadius::same(5))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(&toast.text).color(egui::Color32::WHITE));
                        });
                    ui.add_space(4.0);
                }
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_worker_events();
        self.process_host_messages();
        self.poll_sdl(&ctx);
        self.handle_hotkeys(&ctx);

        match self.screen {
            Screen::Launcher => self.launcher(ui),
            Screen::Game => self.game(ui),
        }

        self.settings_window(&ctx);
        self.controller_window(&ctx);
        self.browser_window(&ctx);
        self.draw_toasts(&ctx);

        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.02, 0.02, 0.03, 1.0]
    }
}

fn config_port_device(config: &Config) -> Option<u32> {
    config.port1_device
}
