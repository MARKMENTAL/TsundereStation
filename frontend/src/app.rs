use crate::clipboard::ClipboardClient;
use crate::config::Config;
use crate::emu::{Command, WorkerEvent, WorkerHandle};
use crate::filebrowser::{BrowseKind, BrowseResult, FileBrowser};
use crate::libretro::ffi;
use crate::libretro::hardware::RendererDecision;
use crate::libretro::host::{HostState, MessageKind};
use crate::libretro::input::PORTS;
use crate::libretro::loader::{state_path, CoreInfo};
use crate::memory_card::MemoryCardManager;
use crate::screenshot;
use crate::sdl_input::{ControllerEntry, NavAction, SdlState};
use crate::shader::{ShaderPreset, ShaderSettings, ShaderState};
use crate::splash::{self, SplashAudio};
use crate::theme::{self, Theme, UiTheme};
use crate::tv_ui::{controller_hint, tv_row};
use eframe::egui;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Splash,
    Launcher,
    Game,
    Settings,
    Controller,
    MemoryCard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavContext {
    FileBrowser,
    MemoryCard,
    Settings,
    Overlay,
    Launcher,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsPane {
    Sidebar,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShaderSliderControl {
    Bloom,
    Scanlines,
    Curvature,
    Mask,
    Vignette,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SettingsEntry {
    ResetAll,
    RestartCore,
    ReloadThemes,
    Theme(String),
    Option(String),
    OptionReset(String),
    ShaderPreset,
    ShaderSlider(ShaderSliderControl),
    ShaderReset,
    StretchToFill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayAction {
    Resume,
    Reset,
    SaveState,
    LoadState,
    TakeScreenshot,
    CopyScreenshot,
    SwapDisc,
    Settings,
    Controller,
    ToggleFullscreen,
    ToggleFps,
    QuitToLauncher,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherItem {
    Play,
    BootBios,
    BrowseGame,
    RecentGame(usize),
    AddBios,
    OpenBios,
    Controller,
    ControllerP2,
    RescanControllers,
    ControllerSettings,
    MemoryCardManager,
    ToggleSplash,
    ToggleFps,
    ToggleR3FastForward,
    ToggleFullscreen,
    Settings,
    Quit,
}

struct Toast {
    text: String,
    kind: MessageKind,
    until: Instant,
}

/// Measures emulated frames per second from the worker's monotonic frame
/// counter. The core-reported rate in `HostState::fps()` is only a pacing
/// target, so it cannot tell us whether the emulator is actually keeping up.
#[derive(Default)]
struct FpsMeter {
    sample: Option<(Instant, u64)>,
    value: f64,
    has_value: bool,
}

impl FpsMeter {
    fn reset(&mut self) {
        self.sample = None;
        self.value = 0.0;
        self.has_value = false;
    }

    /// Drop the current timing baseline but keep the last measured value, so
    /// pause/menu transitions do not make the counter jump to zero.
    fn suspend(&mut self) {
        self.sample = None;
    }

    fn update(&mut self, frames: u64, now: Instant) {
        let Some((sample_at, sample_frames)) = self.sample else {
            self.sample = Some((now, frames));
            return;
        };
        let elapsed = now.saturating_duration_since(sample_at);
        if elapsed < Duration::from_millis(500) {
            return;
        }
        // A long UI/window stall would otherwise produce one bogus sample.
        if elapsed > Duration::from_secs(2) {
            self.sample = Some((now, frames));
            return;
        }
        let delta = frames.saturating_sub(sample_frames);
        let instant = delta as f64 / elapsed.as_secs_f64();
        self.value = if delta == 0 {
            0.0
        } else if self.value > 0.0 {
            self.value * 0.5 + instant * 0.5
        } else {
            instant
        };
        self.has_value = true;
        self.sample = Some((now, frames));
    }

    fn value(&self) -> f64 {
        self.value
    }

    fn display(&self) -> String {
        if self.has_value {
            format!("{:.1} FPS", self.value())
        } else {
            "-- FPS".to_string()
        }
    }
}

pub struct App {
    config: Config,
    host: Arc<HostState>,
    worker: WorkerHandle,
    core_info: Option<CoreInfo>,
    sdl: SdlState,

    screen: Screen,
    screen_stack: Vec<Screen>,
    launcher_focus: usize,
    game_path: Option<String>,
    game_title: String,
    loading_game: bool,
    last_error: Option<String>,

    paused: bool,
    paused_by_overlay: bool,
    overlay_open: bool,
    fast_forward_latched: bool,
    menu_selection: usize,
    disc_labels: Vec<String>,
    disc_index: usize,
    swap_disc_open: bool,
    swap_disc_focus: usize,
    settings_category: String,
    settings_search: String,
    settings_pane: SettingsPane,
    settings_sidebar_focus: usize,
    settings_content_focus: usize,
    memory_card: MemoryCardManager,
    quick_slot: u32,
    fps: FpsMeter,
    shader: Arc<Mutex<ShaderState>>,
    clipboard: ClipboardClient,

    texture: Option<egui::TextureHandle>,
    texture_seq: u64,
    texture_size: [usize; 2],

    logo_texture: Option<egui::TextureHandle>,
    themes: Vec<Theme>,
    ui_theme: UiTheme,
    theme_error: Option<String>,
    splash_audio: Arc<Mutex<SplashAudio>>,
    splash_start: Instant,
    splash_duration: Duration,
    splash_fade_started: bool,

    toasts: Vec<Toast>,
    file_browser: Option<FileBrowser>,
    bios_files: Vec<(String, u64)>,

    port_devices: [Option<u32>; PORTS],
    p2_port_active: bool,
    keyboard_lockout: bool,
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
        worker: WorkerHandle,
        auto_start: bool,
        auto_boot_bios: bool,
        splash_override: Option<bool>,
    ) -> anyhow::Result<Self> {
        host.video.set_context(cc.egui_ctx.clone());

        let (themes, theme_error) = theme::load_catalog();
        let selected_index = themes
            .iter()
            .position(|theme| theme.name == config.theme)
            .unwrap_or(0);
        if themes[selected_index].name != config.theme {
            config.theme = themes[selected_index].name.clone();
            let _ = config.save();
        }
        let ui_theme = themes[selected_index].apply(&cc.egui_ctx);

        let logo_texture = match splash::logo_image() {
            Ok(image) => Some(cc.egui_ctx.load_texture(
                "tsunderestation-logo",
                image,
                egui::TextureOptions::LINEAR,
            )),
            Err(e) => {
                log::warn!("Could not decode the embedded logo: {e:#}");
                None
            }
        };

        let show_splash = splash_override.unwrap_or(config.show_splash);
        let splash_audio = if show_splash {
            match SplashAudio::decode_motif() {
                Ok(audio) => audio,
                Err(e) => {
                    log::warn!("Could not decode the splash motif: {e:#}");
                    SplashAudio::empty()
                }
            }
        } else {
            SplashAudio::empty()
        };
        let splash_audio = Arc::new(Mutex::new(splash_audio));

        let sdl = SdlState::new(
            host.audio.clone(),
            &config.controller_guid,
            &config.controller_guid_p2,
            splash_audio.clone(),
        )?;
        if !sdl.player_guid(0).is_empty() && sdl.player_guid(0) != config.controller_guid {
            config.controller_guid = sdl.player_guid(0).to_string();
            let _ = config.save();
        }

        let fullscreen = config.fullscreen;

        let port_devices = [config.port1_device, config.port2_device];
        let splash_seconds = config.splash_seconds;
        let memory_card = MemoryCardManager::new(
            config.save_dir.clone(),
            config.memory_card_source_dir.clone(),
        );
        let mut app = Self {
            config,
            host,
            worker,
            core_info: None,
            sdl,
            screen: if show_splash {
                Screen::Splash
            } else {
                Screen::Launcher
            },
            screen_stack: Vec::new(),
            launcher_focus: 0,
            game_path: None,
            game_title: String::new(),
            loading_game: false,
            last_error: None,
            paused: false,
            paused_by_overlay: false,
            overlay_open: false,
            fast_forward_latched: false,
            menu_selection: 0,
            disc_labels: Vec::new(),
            disc_index: 0,
            swap_disc_open: false,
            swap_disc_focus: 0,
            settings_category: String::new(),
            settings_search: String::new(),
            settings_pane: SettingsPane::Sidebar,
            settings_sidebar_focus: 0,
            settings_content_focus: 0,
            memory_card,
            quick_slot: 0,
            fps: FpsMeter::default(),
            shader: Arc::new(Mutex::new(ShaderState::default())),
            clipboard: ClipboardClient::default(),
            texture: None,
            texture_seq: 0,
            texture_size: [0, 0],
            logo_texture,
            themes,
            ui_theme,
            theme_error,
            splash_audio,
            splash_start: Instant::now(),
            splash_duration: Duration::from_secs_f32(splash_seconds.clamp(1.0, 30.0)),
            splash_fade_started: false,
            toasts: Vec::new(),
            file_browser: None,
            bios_files: Vec::new(),
            port_devices,
            p2_port_active: false,
            keyboard_lockout: false,
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
                    self.resolve_port_devices(&info);
                    self.core_info = Some(info);
                    self.last_error = None;
                    self.maybe_auto_start();
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
                    self.screen_stack.clear();
                    self.fast_forward_latched = false;
                    self.fps.reset();
                    self.apply_player2_port();
                    self.overlay_open = false;
                    self.disc_labels.clear();
                    self.disc_index = 0;
                    self.swap_disc_open = false;
                    self.paused = false;
                    self.host.input.clear();
                    self.sdl
                        .suppress_input_until_release(Duration::from_millis(1000));
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
                    self.fast_forward_latched = false;
                    self.launcher_focus = 0;
                    self.fps.reset();
                    self.game_path = None;
                    self.loading_game = false;
                    self.overlay_open = false;
                    self.paused = false;
                    self.paused_by_overlay = false;
                    self.screen_stack.clear();
                    self.disc_labels.clear();
                    self.disc_index = 0;
                    self.swap_disc_open = false;
                    self.host.input.clear();
                    self.sdl
                        .suppress_input_until_release(Duration::from_millis(1000));
                    self.keyboard_lockout = true;
                    self.p2_port_active = false;
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
                WorkerEvent::DiscInfo { labels, current } => {
                    self.disc_index = if labels.is_empty() {
                        0
                    } else {
                        current.min(labels.len() - 1)
                    };
                    self.disc_labels = labels;
                    if self.disc_labels.len() <= 1 {
                        self.swap_disc_open = false;
                    }
                    self.swap_disc_focus = self
                        .swap_disc_focus
                        .min(self.disc_labels.len().saturating_sub(1));
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

    fn maybe_auto_start(&mut self) {
        if self.screen == Screen::Splash || self.did_auto_start || self.core_info.is_none() {
            return;
        }
        self.did_auto_start = true;
        if self.auto_boot_bios {
            self.start_bios();
        } else if self.auto_start {
            self.start_game();
        }
    }

    fn finish_splash(&mut self) {
        if self.screen != Screen::Splash {
            return;
        }
        if let Ok(mut audio) = self.splash_audio.lock() {
            audio.stop();
        }
        // Consume the press that dismissed the splash until it is released so
        // it cannot activate a launcher row on the next frame.
        self.host.input.clear();
        self.sdl
            .suppress_input_until_release(Duration::from_millis(1000));
        self.keyboard_lockout = true;
        self.screen = Screen::Launcher;
        self.maybe_auto_start();
    }

    fn handle_splash(&mut self, ctx: &egui::Context) -> bool {
        if self.screen != Screen::Splash {
            return false;
        }
        let elapsed = self.splash_start.elapsed();
        let fade_len = Duration::from_millis(600);
        if !self.splash_fade_started && elapsed + fade_len >= self.splash_duration {
            if let Ok(mut audio) = self.splash_audio.lock() {
                audio.fade_out(fade_len);
            }
            self.splash_fade_started = true;
        }

        let key = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })
        });
        let click = ctx.input(|i| i.pointer.any_click());
        let controller = self.sdl.take_activity();
        if elapsed >= self.splash_duration || key || click || controller {
            self.finish_splash();
            return true;
        }
        false
    }

    fn splash(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::ZERO, self.ui_theme.background);

            let elapsed = self.splash_start.elapsed().as_secs_f32();
            let total = self.splash_duration.as_secs_f32().max(0.1);
            let fade_in = (elapsed / 0.35).clamp(0.0, 1.0);
            let fade_out = ((total - elapsed) / 0.6).clamp(0.0, 1.0);
            let alpha = fade_in.min(fade_out);

            if let Some(texture) = &self.logo_texture {
                let side = rect.width().min(rect.height()) * 0.72;
                let image_rect = egui::Rect::from_center_size(
                    rect.center() - egui::vec2(0.0, 14.0),
                    egui::vec2(side, side),
                );
                let tint = egui::Color32::from_white_alpha((alpha * 255.0) as u8);
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    tint,
                );
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "TsundereStation",
                    egui::FontId::proportional(42.0),
                    self.ui_theme.text.gamma_multiply(alpha),
                );
            }

            let hint = self.ui_theme.muted.gamma_multiply(alpha);
            ui.painter().text(
                rect.center_bottom() - egui::vec2(0.0, 46.0),
                egui::Align2::CENTER_BOTTOM,
                "Press any key or button to skip",
                egui::FontId::proportional(18.0),
                hint,
            );

            let progress = (elapsed / total).clamp(0.0, 1.0);
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.center().x - 200.0, rect.bottom() - 28.0),
                egui::vec2(400.0, 4.0),
            );
            ui.painter()
                .rect_filled(bar, egui::CornerRadius::same(2), self.ui_theme.surface2);
            let fill = egui::Rect::from_min_size(
                bar.min,
                egui::vec2(bar.width() * progress, bar.height()),
            );
            ui.painter()
                .rect_filled(fill, egui::CornerRadius::same(2), self.ui_theme.accent);
        });
    }

    fn resolve_port_devices(&mut self, info: &CoreInfo) {
        let mut changed = false;
        for port in 0..PORTS {
            let Some(port_info) = info.ports.get(port) else {
                self.port_devices[port] = None;
                continue;
            };
            let configured = if port == 0 {
                self.config.port1_device
            } else {
                self.config.port2_device
            };
            let Some(id) = choose_port_device(&port_info.types, configured) else {
                continue;
            };
            if self.port_devices[port] != Some(id) {
                self.port_devices[port] = Some(id);
                changed = true;
            }
            if port == 0 {
                self.config.port1_device = Some(id);
            } else {
                self.config.port2_device = Some(id);
            }
        }
        if changed {
            let _ = self.config.save();
        }
    }

    fn ensure_player2_device(&mut self) {
        if self.port_devices[1].is_none() {
            if let Some(info) = self.core_info.clone() {
                self.resolve_port_devices(&info);
            }
        }
    }

    /// Tell the core whether the second controller port should be enabled.
    /// Keeping it disabled until a Player 2 pad is assigned avoids games
    /// detecting a phantom second player.
    fn sync_player2_port(&mut self) {
        if self.screen != Screen::Game {
            return;
        }
        let present = self.sdl.player_instance(1).is_some();
        let device = if present { self.port_devices[1] } else { None };
        let should_be_active = device.is_some();
        if should_be_active == self.p2_port_active {
            return;
        }
        self.worker.send(Command::SetPortDevice(
            1,
            device.unwrap_or(ffi::RETRO_DEVICE_NONE),
        ));
        self.p2_port_active = should_be_active;
    }

    /// Re-apply the Player 2 port state after a game has booted. This covers
    /// cores that apply controller-port changes only after loading content.
    fn apply_player2_port(&mut self) {
        if self.screen != Screen::Game {
            return;
        }
        let device = if self.sdl.player_instance(1).is_some() {
            self.port_devices[1]
        } else {
            None
        };
        self.worker.send(Command::SetPortDevice(
            1,
            device.unwrap_or(ffi::RETRO_DEVICE_NONE),
        ));
        self.p2_port_active = device.is_some();
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

    fn ui_input_locked(&mut self, ctx: &egui::Context) -> bool {
        if !self.keyboard_lockout {
            return false;
        }
        let any_key_down = ctx.input(|i| !i.keys_down.is_empty());
        if any_key_down {
            return true;
        }
        self.keyboard_lockout = false;
        false
    }

    fn update_measured_fps(&mut self) {
        if self.screen != Screen::Game || self.paused || !self.config.show_fps {
            self.fps.suspend();
            return;
        }
        self.fps.update(self.host.emulated_frames(), Instant::now());
    }

    fn poll_sdl(&mut self, ctx: &egui::Context) {
        let menu = self.sdl.poll(&self.host, &self.config);
        let fast_forward_pressed = self.sdl.take_fast_forward_pressed();
        let p1_guid = self.sdl.player_guid(0).to_string();
        let p2_guid = self.sdl.player_guid(1).to_string();
        let mut dirty = false;
        if !p1_guid.is_empty() && p1_guid != self.config.controller_guid {
            self.config.controller_guid = p1_guid;
            dirty = true;
        }
        if p2_guid != self.config.controller_guid_p2 {
            self.config.controller_guid_p2 = p2_guid;
            dirty = true;
        }
        if dirty {
            let _ = self.config.save();
        }
        self.sync_player2_port();
        if menu {
            match self.screen {
                Screen::Settings | Screen::MemoryCard => self.back_page(),
                Screen::Controller => {}
                Screen::Game => self.toggle_overlay(),
                _ => {}
            }
        }
        if fast_forward_pressed && self.screen == Screen::Game {
            self.fast_forward_latched = !self.fast_forward_latched;
            self.push_toast(
                if self.fast_forward_latched {
                    "Fast-forward on"
                } else {
                    "Fast-forward off"
                }
                .into(),
                MessageKind::Info,
                2.0,
            );
        }

        if self.overlay_open && !self.paused && self.screen == Screen::Game {
            // When the game keeps running behind the menu, do not let menu
            // navigation leak into either emulated pad. While paused we leave
            // the snapshots live so the diagnostics can show them.
            self.host.input.clear();
        }
        let nav_context = if self.file_browser.is_some() {
            Some(NavContext::FileBrowser)
        } else {
            match self.screen {
                Screen::Settings => Some(NavContext::Settings),
                Screen::MemoryCard => Some(NavContext::MemoryCard),
                Screen::Game if self.overlay_open => Some(NavContext::Overlay),
                Screen::Launcher => Some(NavContext::Launcher),
                _ => None,
            }
        };
        for nav in self.sdl.poll_menu_nav(nav_context.is_some()) {
            match nav_context {
                Some(NavContext::FileBrowser) => self.handle_file_browser_nav(nav),
                Some(NavContext::MemoryCard) => {
                    if self.memory_card.navigate(nav) {
                        self.back_page();
                    }
                }
                Some(NavContext::Settings) => self.handle_settings_nav(nav, ctx),
                Some(NavContext::Overlay) => self.handle_overlay_nav(nav, ctx),
                Some(NavContext::Launcher) => self.handle_launcher_nav(nav, ctx),
                None => {}
            }
        }
    }

    fn handle_frontend_keys(&mut self, ctx: &egui::Context) {
        if self.file_browser.is_some() {
            if let Some(nav) = key_to_nav(ctx) {
                self.handle_file_browser_nav(nav);
            }
            return;
        }
        match self.screen {
            Screen::MemoryCard => {
                if !ctx.egui_wants_keyboard_input() {
                    if let Some(nav) = key_to_nav(ctx) {
                        if self.memory_card.navigate(nav) {
                            self.back_page();
                            consume_escape(ctx);
                        }
                    }
                }
            }
            Screen::Settings => {
                if !ctx.egui_wants_keyboard_input() {
                    if let Some(nav) = key_to_nav(ctx) {
                        self.handle_settings_nav(nav, ctx);
                    }
                    if self.screen != Screen::Settings {
                        consume_escape(ctx);
                    }
                }
            }
            Screen::Controller => {
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.back_page();
                    consume_escape(ctx);
                }
            }
            Screen::Launcher => {
                if !ctx.egui_wants_keyboard_input() {
                    if let Some(nav) = key_to_nav(ctx) {
                        self.handle_launcher_nav(nav, ctx);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_file_browser_nav(&mut self, nav: NavAction) {
        let mut result = None;
        let mut kind = None;
        if let Some(browser) = self.file_browser.as_mut() {
            kind = Some(browser.kind);
            result = browser.navigate(nav);
        }
        if let Some(result) = result {
            self.file_browser = None;
            if let Some(kind) = kind {
                self.apply_browse_result(result, kind);
            }
        }
    }

    fn overlay_actions(&self) -> Vec<(OverlayAction, String)> {
        let mut actions = vec![
            (OverlayAction::Resume, "Resume".to_string()),
            (OverlayAction::Reset, "Reset console".to_string()),
            (
                OverlayAction::SaveState,
                format!("Save state (slot {})", self.quick_slot),
            ),
            (
                OverlayAction::LoadState,
                format!("Load state (slot {})", self.quick_slot),
            ),
            (
                OverlayAction::TakeScreenshot,
                "Take screenshot (F12)".to_string(),
            ),
            (
                OverlayAction::CopyScreenshot,
                "Copy screenshot to clipboard".to_string(),
            ),
        ];
        if self.disc_labels.len() > 1 {
            let current = self
                .disc_index
                .saturating_add(1)
                .min(self.disc_labels.len());
            actions.push((
                OverlayAction::SwapDisc,
                format!("Swap disc… ({current}/{})", self.disc_labels.len()),
            ));
        }
        actions.extend([
            (OverlayAction::Settings, "Settings…".to_string()),
            (OverlayAction::Controller, "Controller…".to_string()),
            (
                OverlayAction::ToggleFullscreen,
                "Toggle fullscreen (F11)".to_string(),
            ),
            (
                OverlayAction::ToggleFps,
                if self.config.show_fps {
                    "Hide FPS display (F3)".to_string()
                } else {
                    "Show FPS display (F3)".to_string()
                },
            ),
            (
                OverlayAction::QuitToLauncher,
                "Quit to launcher".to_string(),
            ),
            (OverlayAction::Quit, "Quit TsundereStation".to_string()),
        ]);
        actions
    }

    fn handle_overlay_nav(&mut self, nav: NavAction, ctx: &egui::Context) {
        if self.swap_disc_open {
            let count = self.disc_labels.len();
            if count == 0 {
                self.swap_disc_open = false;
                return;
            }
            match nav {
                NavAction::Up => {
                    self.swap_disc_focus = (self.swap_disc_focus + count - 1) % count;
                }
                NavAction::Down => {
                    self.swap_disc_focus = (self.swap_disc_focus + 1) % count;
                }
                NavAction::Confirm => self.request_swap_disc(self.swap_disc_focus),
                NavAction::Cancel => self.swap_disc_open = false,
                NavAction::Left | NavAction::Right | NavAction::PrevTab | NavAction::NextTab => {}
            }
            return;
        }

        let actions = self.overlay_actions();
        if actions.is_empty() {
            return;
        }
        if self.menu_selection >= actions.len() {
            self.menu_selection = 0;
        }
        match nav {
            NavAction::Up => {
                self.menu_selection = (self.menu_selection + actions.len() - 1) % actions.len();
            }
            NavAction::Down => {
                self.menu_selection = (self.menu_selection + 1) % actions.len();
            }
            NavAction::Left => {
                self.quick_slot = (self.quick_slot + 9) % 10;
            }
            NavAction::Right => {
                self.quick_slot = (self.quick_slot + 1) % 10;
            }
            NavAction::Cancel => self.close_overlay(),
            NavAction::PrevTab | NavAction::NextTab => {}
            NavAction::Confirm => {
                let action = actions[self.menu_selection].0;
                self.activate_overlay_action(action, ctx);
            }
        }
    }

    fn activate_overlay_action(&mut self, action: OverlayAction, ctx: &egui::Context) {
        match action {
            OverlayAction::Resume => self.close_overlay(),
            OverlayAction::Reset => {
                self.worker.send(Command::Reset);
                self.close_overlay();
            }
            OverlayAction::SaveState => self.worker.send(Command::SaveState(self.quick_slot)),
            OverlayAction::LoadState => self.worker.send(Command::LoadState(self.quick_slot)),
            OverlayAction::TakeScreenshot => self.take_screenshot(),
            OverlayAction::CopyScreenshot => self.copy_screenshot(),
            OverlayAction::SwapDisc => self.open_swap_disc_picker(),
            OverlayAction::Settings => self.open_settings(),
            OverlayAction::Controller => self.open_controller(),
            OverlayAction::ToggleFullscreen => self.toggle_fullscreen(ctx),
            OverlayAction::ToggleFps => self.toggle_fps_display(true),
            OverlayAction::QuitToLauncher => self.quit_to_launcher(),
            OverlayAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn open_swap_disc_picker(&mut self) {
        if self.disc_labels.len() <= 1 {
            return;
        }
        self.swap_disc_focus = self.disc_index.min(self.disc_labels.len() - 1);
        self.swap_disc_open = true;
    }

    fn request_swap_disc(&mut self, index: usize) {
        self.swap_disc_open = false;
        if index >= self.disc_labels.len() {
            return;
        }
        if index == self.disc_index {
            return;
        }
        self.worker.send(Command::SwapDisc(index));
        self.push_toast(
            format!("Inserting disc {}…", index + 1),
            MessageKind::Info,
            2.0,
        );
    }

    fn swap_disc_window(&mut self, ctx: &egui::Context) {
        if !self.swap_disc_open {
            return;
        }
        let labels = self.disc_labels.clone();
        if labels.len() <= 1 {
            self.swap_disc_open = false;
            return;
        }
        let current = self.disc_index.min(labels.len() - 1);
        let mut focus = self.swap_disc_focus.min(labels.len() - 1);
        let mut open = true;
        let mut close = false;
        let mut pick = None;
        let window_frame = theme::window_frame(&ctx.global_style(), &self.ui_theme);

        egui::Window::new("Select disc")
            .id(egui::Id::new("disc-picker"))
            .open(&mut open)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .frame(window_frame)
            .show(ctx, |ui| {
                if theme::window_header(ui, "Select disc", &self.ui_theme) {
                    close = true;
                }
                theme::window_body(ui, |ui| {
                    ui.set_min_width(620.0);
                    ui.weak(format!(
                        "Current disc: {} — {}",
                        current + 1,
                        labels[current]
                    ));
                    ui.separator();
                    for (index, label) in labels.iter().enumerate() {
                        let selected = focus == index;
                        let text = if index == current {
                            format!("{label}  (current)")
                        } else {
                            label.clone()
                        };
                        let response =
                            tv_row(ui, selected, &text, &self.ui_theme, ("disc-picker", index));
                        if response.clicked() {
                            focus = index;
                            pick = Some(index);
                        }
                        if selected {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button("Insert disc").clicked() {
                            pick = Some(focus);
                        }
                    });
                    controller_hint(
                        ui,
                        "D-pad / left stick: choose  ·  Cross: insert  ·  Circle/PS: cancel",
                        &self.ui_theme,
                    );
                });
            });

        self.swap_disc_focus = focus;
        if close || pick.is_some() {
            self.swap_disc_open = false;
        }
        if let Some(index) = pick {
            self.request_swap_disc(index);
        }
    }

    fn take_screenshot(&mut self) {
        let Some((frame, _)) = self.host.video.latest() else {
            self.push_toast(
                "No video frame is available yet".into(),
                MessageKind::Warning,
                3.0,
            );
            return;
        };
        let raw_name = if self.game_title.is_empty() {
            self.game_path.as_deref().unwrap_or("tsunderestation")
        } else {
            &self.game_title
        };
        match screenshot::capture(&frame, raw_name, &crate::config::screenshots_dir()) {
            Ok(path) => self.push_toast(
                format!("Screenshot saved to {}", path.display()),
                MessageKind::Info,
                4.0,
            ),
            Err(error) => self.push_toast(
                format!("Could not save screenshot: {error:#}"),
                MessageKind::Error,
                6.0,
            ),
        }
    }

    fn copy_screenshot(&mut self) {
        let Some((frame, _)) = self.host.video.latest() else {
            self.push_toast(
                "No video frame is available yet".into(),
                MessageKind::Warning,
                3.0,
            );
            return;
        };
        match self.clipboard.copy_frame(frame) {
            Ok(()) => self.push_toast(
                "Copying screenshot to clipboard…".into(),
                MessageKind::Info,
                2.0,
            ),
            Err(error) => self.push_toast(
                format!("Could not copy screenshot: {error:#}"),
                MessageKind::Error,
                6.0,
            ),
        }
    }

    fn poll_clipboard(&mut self) {
        while let Some(result) = self.clipboard.poll() {
            match result {
                Ok(()) => self.push_toast(
                    "Screenshot copied to clipboard".into(),
                    MessageKind::Info,
                    3.0,
                ),
                Err(error) => self.push_toast(
                    format!("Could not copy screenshot: {error:#}"),
                    MessageKind::Error,
                    6.0,
                ),
            }
        }
    }

    fn toggle_fps_display(&mut self, notify: bool) {
        self.config.show_fps = !self.config.show_fps;
        let _ = self.config.save();
        if notify {
            self.push_toast(
                if self.config.show_fps {
                    "FPS/status overlay on"
                } else {
                    "FPS/status overlay off"
                }
                .into(),
                MessageKind::Info,
                2.0,
            );
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        if self.screen != Screen::Game {
            return;
        }
        // Game hotkeys are disabled while one of the frontend's text-entry
        // windows is open. Selectable labels can take focus too, so don't use
        // egui_wants_keyboard_input() here.
        let typing = self.file_browser.is_some();
        if !typing {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F1)) {
                if self.swap_disc_open {
                    self.swap_disc_open = false;
                } else {
                    self.toggle_overlay();
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
                self.worker.send(Command::SaveState(self.quick_slot));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
                self.worker.send(Command::LoadState(self.quick_slot));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
                self.toggle_fps_display(true);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
                self.toggle_fullscreen(ctx);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
                self.take_screenshot();
            }
            if self.overlay_open {
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
        let fast = fast_forward_enabled(
            self.fast_forward_latched,
            ctx.input(|i| i.key_down(egui::Key::Tab)),
            self.overlay_open,
            typing,
        );
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
        self.swap_disc_open = false;
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
        self.swap_disc_open = false;

        if self.screen == Screen::Game {
            // Drop the button press that selected Resume/Close immediately so
            // the core cannot observe it on the first resumed frame, then keep
            // suppressing until the pad returns to neutral.
            self.host.input.clear();
            self.sdl
                .suppress_input_until_release(Duration::from_millis(1000));
        }

        if self.paused_by_overlay {
            self.paused = false;
            self.paused_by_overlay = false;
            self.worker.send(Command::Pause(false));
        }
    }

    fn load_port_devices(&self) -> [Option<u32>; PORTS] {
        let p2 = if self.sdl.player_instance(1).is_some() {
            self.port_devices[1]
        } else {
            None
        };
        [self.port_devices[0], p2]
    }

    fn start_bios(&mut self) {
        if self.core_info.is_none() {
            return;
        }
        self.host.input.clear();
        self.sdl
            .suppress_input_until_release(Duration::from_millis(1000));
        self.keyboard_lockout = true;
        self.loading_game = true;
        self.last_error = None;
        let port_devices = self.load_port_devices();
        self.p2_port_active = port_devices[1].is_some();
        self.worker.send(Command::BootBios { port_devices });
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
        self.host.input.clear();
        self.sdl
            .suppress_input_until_release(Duration::from_millis(1000));
        self.keyboard_lockout = true;
        self.loading_game = true;
        self.last_error = None;
        let port_devices = self.load_port_devices();
        self.p2_port_active = port_devices[1].is_some();
        self.worker.send(Command::LoadGame { path, port_devices });
    }

    fn quit_to_launcher(&mut self) {
        self.worker.send(Command::Pause(false));
        self.worker.send(Command::UnloadGame);
        self.host
            .fast_forward
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.screen = Screen::Launcher;
        self.screen_stack.clear();
        self.fast_forward_latched = false;
        self.launcher_focus = 0;
        self.overlay_open = false;
        self.paused = false;
        self.paused_by_overlay = false;
        self.loading_game = false;
        self.fps.reset();
        self.host.input.clear();
        self.sdl
            .suppress_input_until_release(Duration::from_millis(1000));
        self.keyboard_lockout = true;
        self.p2_port_active = false;
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

    fn launcher_items(&self) -> Vec<LauncherItem> {
        let mut items = vec![
            LauncherItem::Play,
            LauncherItem::BootBios,
            LauncherItem::BrowseGame,
        ];
        for index in 0..self.config.recent_games.len() {
            items.push(LauncherItem::RecentGame(index));
        }
        items.extend([
            LauncherItem::AddBios,
            LauncherItem::OpenBios,
            LauncherItem::Controller,
            LauncherItem::ControllerP2,
            LauncherItem::RescanControllers,
            LauncherItem::ControllerSettings,
            LauncherItem::MemoryCardManager,
            LauncherItem::ToggleSplash,
            LauncherItem::ToggleFps,
            LauncherItem::ToggleR3FastForward,
            LauncherItem::ToggleFullscreen,
            LauncherItem::Settings,
            LauncherItem::Quit,
        ]);
        items
    }

    fn launcher_label(&self, item: LauncherItem) -> String {
        match item {
            LauncherItem::Play => "Play".to_string(),
            LauncherItem::BootBios => "Boot BIOS".to_string(),
            LauncherItem::BrowseGame => "Browse game…".to_string(),
            LauncherItem::RecentGame(index) => {
                let path = self
                    .config
                    .recent_games
                    .get(index)
                    .map(String::as_str)
                    .unwrap_or("Unknown game");
                format!("Recent: {}", short_game_name(path))
            }
            LauncherItem::AddBios => "Add BIOS image…".to_string(),
            LauncherItem::OpenBios => "Open BIOS folder".to_string(),
            LauncherItem::Controller => format!(
                "Player 1 controller: {}  (Left/Right)",
                self.sdl.player_name(0).unwrap_or("None detected")
            ),
            LauncherItem::ControllerP2 => {
                let name = self.sdl.player_name(1).unwrap_or_else(|| {
                    if self.sdl.player_guid(1).is_empty() {
                        "None"
                    } else {
                        "Disconnected"
                    }
                });
                format!("Player 2 controller: {name}  (Left/Right)")
            }
            LauncherItem::RescanControllers => "Rescan controllers".to_string(),
            LauncherItem::ControllerSettings => "Controller settings…".to_string(),
            LauncherItem::MemoryCardManager => "Memory Card Manager…".to_string(),
            LauncherItem::ToggleSplash => format!(
                "Show splash on startup: {}",
                if self.config.show_splash { "On" } else { "Off" }
            ),
            LauncherItem::ToggleFps => format!(
                "FPS/status overlay: {}",
                if self.config.show_fps { "On" } else { "Off" }
            ),
            LauncherItem::ToggleR3FastForward => format!(
                "R3 fast-forward: {}",
                if self.config.r3_fast_forward {
                    "On"
                } else {
                    "Off"
                }
            ),
            LauncherItem::ToggleFullscreen => "Toggle fullscreen".to_string(),
            LauncherItem::Settings => "Settings…".to_string(),
            LauncherItem::Quit => "Quit TsundereStation".to_string(),
        }
    }

    fn open_page(&mut self, screen: Screen) {
        if self.screen == screen {
            return;
        }
        self.screen_stack.push(self.screen);
        self.screen = screen;
    }

    fn back_page(&mut self) {
        self.screen = self.screen_stack.pop().unwrap_or(Screen::Launcher);
    }

    fn open_settings(&mut self) {
        self.settings_pane = SettingsPane::Sidebar;
        self.settings_sidebar_focus = self
            .settings_categories()
            .iter()
            .position(|(key, _)| key == &self.settings_category)
            .unwrap_or(0);
        self.settings_content_focus = 0;
        self.open_page(Screen::Settings);
    }

    fn open_controller(&mut self) {
        self.open_page(Screen::Controller);
    }

    fn open_memory_card(&mut self) {
        self.memory_card.open();
        self.open_page(Screen::MemoryCard);
    }

    fn cycle_controller(&mut self, player: usize) {
        self.cycle_controller_dir(player, 1);
    }

    fn cycle_controller_dir(&mut self, player: usize, delta: i32) {
        if player >= PORTS {
            return;
        }
        let other = 1 - player;
        let other_instance = self.sdl.player_instance(other);
        let candidates: Vec<ControllerEntry> = self
            .sdl
            .controllers()
            .iter()
            .filter(|entry| Some(entry.instance_id) != other_instance)
            .cloned()
            .collect();
        if candidates.is_empty() {
            let message = if player == 0 {
                "No controller detected"
            } else {
                "No second controller detected"
            };
            self.push_toast(message.into(), MessageKind::Warning, 3.0);
            return;
        }

        let mut choices: Vec<Option<ControllerEntry>> = Vec::new();
        if player == 1 {
            choices.push(None);
        }
        choices.extend(candidates.into_iter().map(Some));

        let current_instance = self.sdl.player_instance(player);
        let current = choices
            .iter()
            .position(|choice| choice.as_ref().map(|entry| entry.instance_id) == current_instance)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(choices.len() as i32) as usize;

        match &choices[next] {
            Some(entry) => {
                let instance = entry.instance_id;
                let guid = entry.guid.clone();
                self.sdl.set_player_instance(player, Some(instance));
                if player == 0 {
                    self.config.controller_guid = guid;
                } else {
                    self.config.controller_guid_p2 = guid;
                }
            }
            None => {
                self.sdl.set_player_instance(player, None);
                self.config.controller_guid_p2.clear();
            }
        }
        self.ensure_player2_device();
        let _ = self.config.save();
        self.sync_player2_port();
    }

    fn open_browser(&mut self, kind: BrowseKind) {
        let start = match kind {
            BrowseKind::Game => {
                let path = Path::new(&self.config.last_game);
                if path.is_file() {
                    path.parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(crate::config::home_dir)
                } else {
                    crate::config::home_dir()
                }
            }
            BrowseKind::Bios => crate::config::home_dir(),
            BrowseKind::MemoryCard => crate::config::home_dir(),
        };
        let extensions = match kind {
            BrowseKind::Game => self
                .core_info
                .as_ref()
                .map(|info| {
                    info.extensions
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
                }),
            BrowseKind::Bios => vec!["bin".into(), "rom".into(), "img".into()],
            BrowseKind::MemoryCard => {
                vec!["srm".into(), "mcd".into(), "mcr".into(), "gme".into()]
            }
        };
        self.file_browser = Some(FileBrowser::new(kind, &start, extensions));
    }

    fn activate_launcher_item(&mut self, item: LauncherItem, ctx: &egui::Context) {
        match item {
            LauncherItem::Play => self.start_game(),
            LauncherItem::BootBios => self.start_bios(),
            LauncherItem::BrowseGame => self.open_browser(BrowseKind::Game),
            LauncherItem::RecentGame(index) => {
                if let Some(path) = self.config.recent_games.get(index).cloned() {
                    self.config.last_game = path;
                    let _ = self.config.save();
                    self.start_game();
                }
            }
            LauncherItem::AddBios => self.open_browser(BrowseKind::Bios),
            LauncherItem::OpenBios => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&self.config.bios_dir)
                    .spawn();
            }
            LauncherItem::Controller | LauncherItem::ControllerP2 => {
                self.open_controller();
            }
            LauncherItem::RescanControllers => {
                self.sdl.refresh_controllers();
                self.ensure_player2_device();
                self.push_toast("Controllers rescanned".into(), MessageKind::Info, 2.0);
            }
            LauncherItem::ControllerSettings => self.open_controller(),
            LauncherItem::MemoryCardManager => self.open_memory_card(),
            LauncherItem::ToggleSplash => {
                self.config.show_splash = !self.config.show_splash;
                let _ = self.config.save();
            }
            LauncherItem::ToggleFps => self.toggle_fps_display(false),
            LauncherItem::ToggleR3FastForward => {
                self.config.r3_fast_forward = !self.config.r3_fast_forward;
                if !self.config.r3_fast_forward {
                    self.fast_forward_latched = false;
                    self.host
                        .fast_forward
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = self.config.save();
            }
            LauncherItem::ToggleFullscreen => self.toggle_fullscreen(ctx),
            LauncherItem::Settings => self.open_settings(),
            LauncherItem::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn handle_launcher_nav(&mut self, nav: NavAction, ctx: &egui::Context) {
        let items = self.launcher_items();
        if items.is_empty() {
            return;
        }
        if self.launcher_focus >= items.len() {
            self.launcher_focus = 0;
        }
        match nav {
            NavAction::Up => {
                self.launcher_focus = (self.launcher_focus + items.len() - 1) % items.len();
            }
            NavAction::Down => {
                self.launcher_focus = (self.launcher_focus + 1) % items.len();
            }
            NavAction::Left | NavAction::Right => match items[self.launcher_focus] {
                LauncherItem::Controller => self.cycle_controller(0),
                LauncherItem::ControllerP2 => self.cycle_controller(1),
                _ => {}
            },
            NavAction::Confirm => {
                let item = items[self.launcher_focus];
                self.activate_launcher_item(item, ctx);
            }
            NavAction::Cancel | NavAction::PrevTab | NavAction::NextTab => {}
        }
    }

    fn launcher(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let items = self.launcher_items();
        if self.launcher_focus >= items.len() {
            self.launcher_focus = items.len().saturating_sub(1);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if self.file_browser.is_none() {
                egui::Panel::bottom("launcher-hint")
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        controller_hint(
                            ui,
                            "D-pad / left stick: move  ·  Cross: select  ·  Left/Right: change player controller",
                            &self.ui_theme,
                        );
                    });
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let max_width = 1100.0f32;
                    let available = ui.available_width();
                    let side = ((available - max_width) * 0.5).max(0.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_max_width(max_width.min(available));
                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                if let Some(texture_id) =
                                    self.logo_texture.as_ref().map(|t| t.id())
                                {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(84.0, 84.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().image(
                                        texture_id,
                                        rect,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.heading("TsundereStation");
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "A SwanStation frontend in Rust · v{}",
                                            env!("CARGO_PKG_VERSION")
                                        ))
                                        .weak()
                                        .size(18.0),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if let Some(info) = &self.core_info {
                                            ui.weak(format!(
                                                "{} core {}",
                                                info.name, info.version
                                            ));
                                        } else {
                                            ui.spinner();
                                            ui.weak("Loading core…");
                                        }
                                    },
                                );
                            });

                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("Game path")
                                    .strong()
                                    .color(self.ui_theme.text),
                            );
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.config.last_game,
                                    )
                                    .desired_width(700.0)
                                    .hint_text("/path/to/game.cue"),
                                );
                            });
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "BIOS: {} image(s) in {}",
                                    self.bios_files.len(),
                                    self.config.bios_dir.display()
                                ))
                                .small()
                                .weak(),
                            );
                            ui.separator();

                            for (index, item) in items.iter().enumerate() {
                                if matches!(
                                    item,
                                    LauncherItem::AddBios
                                        | LauncherItem::Controller
                                        | LauncherItem::ControllerP2
                                        | LauncherItem::MemoryCardManager
                                        | LauncherItem::ToggleSplash
                                        | LauncherItem::Settings
                                ) {
                                    ui.separator();
                                }
                                let focused = index == self.launcher_focus;
                                let label = self.launcher_label(*item);
                                let response = tv_row(ui, focused, &label, &self.ui_theme, ("launcher", index));
                                if response.clicked() {
                                    self.launcher_focus = index;
                                    self.activate_launcher_item(*item, &ctx);
                                }
                                if focused {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                            }

                            ui.add_space(12.0);
                        });
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
                let image_rect =
                    game_image_rect(rect, self.texture_size, self.config.stretch_to_fill);
                let shader_settings = ShaderSettings::from_config(&self.config);
                let shader_failed = self.shader.lock().unwrap().error().is_some();
                if shader_settings.preset != ShaderPreset::Off && !shader_failed {
                    let shader = self.shader.clone();
                    let texture_id = texture.id();
                    let game_size = self.texture_size;
                    let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
                        if let Ok(mut state) = shader.lock() {
                            if let Err(e) =
                                state.paint(painter, info, texture_id, game_size, shader_settings)
                            {
                                log::warn!("shader: {e:#}");
                            }
                        }
                    });
                    ui.painter().add(egui::Shape::Callback(egui::PaintCallback {
                        rect: image_rect,
                        callback: Arc::new(callback),
                    }));
                } else {
                    ui.painter().image(
                        texture.id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Starting…",
                    egui::FontId::proportional(24.0),
                    self.ui_theme.muted,
                );
            }

            let fast_forward = self
                .host
                .fast_forward
                .load(std::sync::atomic::Ordering::Relaxed);
            let status = if self.config.show_fps {
                let mut info = self.fps.display();
                info.push_str(if self.host.hardware_active() {
                    " · HW"
                } else {
                    " · SW"
                });
                if self.paused {
                    info.push_str(" · paused");
                }
                if fast_forward {
                    info.push_str(" · >>");
                }
                Some(info)
            } else if fast_forward {
                Some(">>".to_string())
            } else {
                None
            };
            if let Some(info) = status {
                ui.painter().text(
                    rect.left_top() + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    info,
                    egui::FontId::monospace(17.0),
                    self.ui_theme.muted,
                );
            }
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
        let fps = self.fps.display();
        let state_identity = self
            .game_path
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new("bios"));
        let state_exists =
            state_path(&self.config.state_dir, state_identity, self.quick_slot).exists();
        let actions = self.overlay_actions();
        if self.menu_selection >= actions.len() {
            self.menu_selection = 0;
        }
        let mut clicked = None;

        egui::Window::new(title)
            .id(egui::Id::new("game-menu"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(620.0);
                ui.heading(if self.paused { "Paused" } else { "Menu" });
                ui.weak(if self.config.show_fps {
                    format!("{fps} · slot {}", self.quick_slot)
                } else {
                    format!("slot {}", self.quick_slot)
                });
                if self.disc_labels.len() > 1 {
                    let current = self.disc_index.min(self.disc_labels.len() - 1);
                    ui.label(format!(
                        "Disc {}/{}: {}",
                        current + 1,
                        self.disc_labels.len(),
                        self.disc_labels[current]
                    ));
                }
                ui.horizontal(|ui| {
                    ui.label("Slot");
                    egui::ComboBox::from_id_salt("save-slot")
                        .selected_text(self.quick_slot.to_string())
                        .width(80.0)
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
                egui::ScrollArea::vertical()
                    .max_height(570.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, (action, label)) in actions.iter().enumerate() {
                            let focused = self.menu_selection == index;
                            let response =
                                tv_row(ui, focused, label, &self.ui_theme, ("overlay", index));
                            if response.clicked() {
                                self.menu_selection = index;
                                clicked = Some((index, *action));
                            }
                            if focused {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });
                ui.separator();
                controller_hint(
                    ui,
                    "D-pad/stick: move  ·  Cross: select  ·  Circle/PS: close  ·  Left/Right: slot  ·  R3: fast-forward",
                    &self.ui_theme,
                );
            });

        if let Some((index, action)) = clicked {
            self.menu_selection = index;
            self.activate_overlay_action(action, ctx);
        }
        if self.swap_disc_open {
            self.swap_disc_window(ctx);
        }
    }

    fn settings_categories(&self) -> Vec<(String, String)> {
        let mut categories = vec![
            ("appearance".to_string(), "Appearance".to_string()),
            ("video".to_string(), "Video".to_string()),
            (String::new(), "All options".to_string()),
            ("uncategorized".to_string(), "Uncategorized".to_string()),
        ];
        if let Some(info) = &self.core_info {
            for category in &info.categories {
                categories.push((
                    category.key.clone(),
                    settings_category_label(&category.key, &category.label).to_string(),
                ));
            }
        }
        categories
    }

    fn settings_visible_option_keys(&self) -> Vec<String> {
        let Some(info) = &self.core_info else {
            return Vec::new();
        };
        let store = self.host.options.read().unwrap();
        let search = self.settings_search.to_lowercase();
        let category_keys: Vec<&str> = info
            .categories
            .iter()
            .map(|category| category.key.as_str())
            .collect();
        let mut keys = Vec::new();
        for option in &info.options {
            if option.key == "swanstation_GPU_ResolutionScale" {
                continue;
            }
            if !store.visible.get(&option.key).copied().unwrap_or(true) {
                continue;
            }
            let category_matches = self.settings_category.is_empty()
                || (self.settings_category == "uncategorized"
                    && (option.category_key.is_empty()
                        || !category_keys.contains(&option.category_key.as_str())))
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
            keys.push(option.key.clone());
        }
        keys
    }

    fn settings_entries(&self) -> Vec<SettingsEntry> {
        match self.settings_category.as_str() {
            "appearance" => {
                let search = self.settings_search.to_lowercase();
                let mut entries = vec![SettingsEntry::ReloadThemes];
                for theme in &self.themes {
                    if search.is_empty() || theme.name.to_lowercase().contains(&search) {
                        entries.push(SettingsEntry::Theme(theme.name.clone()));
                    }
                }
                entries
            }
            "video" => vec![
                SettingsEntry::ShaderPreset,
                SettingsEntry::ShaderSlider(ShaderSliderControl::Bloom),
                SettingsEntry::ShaderSlider(ShaderSliderControl::Scanlines),
                SettingsEntry::ShaderSlider(ShaderSliderControl::Curvature),
                SettingsEntry::ShaderSlider(ShaderSliderControl::Mask),
                SettingsEntry::ShaderSlider(ShaderSliderControl::Vignette),
                SettingsEntry::ShaderReset,
            ],
            _ => {
                let mut entries = vec![SettingsEntry::ResetAll, SettingsEntry::RestartCore];
                if self.settings_category == "display" {
                    entries.push(SettingsEntry::StretchToFill);
                }
                let overrides = self.host.options.read().unwrap().overrides.clone();
                for key in self.settings_visible_option_keys() {
                    entries.push(SettingsEntry::Option(key.clone()));
                    if overrides.contains_key(&key) {
                        entries.push(SettingsEntry::OptionReset(key));
                    }
                }
                entries
            }
        }
    }

    fn option_values(&self, key: &str) -> Vec<(String, String)> {
        self.core_info
            .as_ref()
            .and_then(|info| info.options.iter().find(|option| option.key == key))
            .map(|option| option.values.clone())
            .unwrap_or_default()
    }

    fn persist_core_options(&mut self, restart_on_launcher: bool) {
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
        if restart_on_launcher && self.screen == Screen::Launcher && !self.loading_game {
            self.worker.send(Command::RestartCore);
            self.push_toast("Applying settings…".into(), MessageKind::Info, 2.0);
        }
    }

    fn cycle_option_value(&mut self, key: &str, delta: i32) {
        let values = self.option_values(key);
        if values.is_empty() {
            return;
        }
        let current = self.host.option_value(key).unwrap_or_default();
        let index = values
            .iter()
            .position(|(value, _)| value == &current)
            .unwrap_or(0) as i32;
        let next = (index + delta).rem_euclid(values.len() as i32) as usize;
        self.host.set_option(key, values[next].0.clone());
        self.persist_core_options(true);
    }

    fn reset_option_value(&mut self, key: &str) {
        self.host.reset_option(key);
        self.persist_core_options(true);
    }

    fn reset_all_options(&mut self) {
        self.host.options.write().unwrap().reset_all();
        self.persist_core_options(true);
    }

    fn apply_shader_preset(&mut self, preset: ShaderPreset) {
        self.config.shader_preset = preset.as_str().to_string();
        let defaults = preset.defaults();
        self.config.shader_bloom = defaults.bloom;
        self.config.shader_scanlines = defaults.scanlines;
        self.config.shader_curvature = defaults.curvature;
        self.config.shader_mask = defaults.mask;
        self.config.shader_vignette = defaults.vignette;
        self.shader.lock().unwrap().clear_error();
        let _ = self.config.save();
    }

    fn adjust_shader_slider(&mut self, control: ShaderSliderControl, delta: i32) {
        let (value, step, min, max) = match control {
            ShaderSliderControl::Bloom => (&mut self.config.shader_bloom, 0.05, 0.0, 2.0),
            ShaderSliderControl::Scanlines => (&mut self.config.shader_scanlines, 0.05, 0.0, 1.0),
            ShaderSliderControl::Curvature => (&mut self.config.shader_curvature, 0.005, 0.0, 0.25),
            ShaderSliderControl::Mask => (&mut self.config.shader_mask, 0.05, 0.0, 1.0),
            ShaderSliderControl::Vignette => (&mut self.config.shader_vignette, 0.05, 0.0, 1.0),
        };
        *value = (*value + delta as f32 * step).clamp(min, max);
        let _ = self.config.save();
    }

    fn reset_shader_defaults(&mut self) {
        let preset = ShaderPreset::parse(&self.config.shader_preset);
        self.apply_shader_preset(preset);
    }

    fn cycle_shader_preset(&mut self, delta: i32) {
        let current = ShaderPreset::parse(&self.config.shader_preset);
        let index = ShaderPreset::ALL
            .iter()
            .position(|preset| *preset == current)
            .unwrap_or(0) as i32;
        let next = (index + delta).rem_euclid(ShaderPreset::ALL.len() as i32) as usize;
        self.apply_shader_preset(ShaderPreset::ALL[next]);
    }

    fn handle_settings_nav(&mut self, nav: NavAction, ctx: &egui::Context) {
        let categories = self.settings_categories();
        match self.settings_pane {
            SettingsPane::Sidebar => match nav {
                NavAction::Up => {
                    if !categories.is_empty() {
                        self.settings_sidebar_focus =
                            (self.settings_sidebar_focus + categories.len() - 1) % categories.len();
                        self.settings_category = categories[self.settings_sidebar_focus].0.clone();
                    }
                }
                NavAction::Down => {
                    if !categories.is_empty() {
                        self.settings_sidebar_focus =
                            (self.settings_sidebar_focus + 1) % categories.len();
                        self.settings_category = categories[self.settings_sidebar_focus].0.clone();
                    }
                }
                NavAction::Right | NavAction::Confirm => {
                    if let Some((key, _)) = categories.get(self.settings_sidebar_focus) {
                        self.settings_category = key.clone();
                        self.settings_pane = SettingsPane::Content;
                        self.settings_content_focus = 0;
                    }
                }
                NavAction::Left | NavAction::PrevTab | NavAction::NextTab => {}
                NavAction::Cancel => self.back_page(),
            },
            SettingsPane::Content => {
                let entries = self.settings_entries();
                if entries.is_empty() {
                    self.settings_pane = SettingsPane::Sidebar;
                    return;
                }
                self.settings_content_focus = self.settings_content_focus.min(entries.len() - 1);
                match nav {
                    NavAction::Up => {
                        self.settings_content_focus =
                            (self.settings_content_focus + entries.len() - 1) % entries.len();
                    }
                    NavAction::Down => {
                        self.settings_content_focus =
                            (self.settings_content_focus + 1) % entries.len();
                    }
                    NavAction::Left => self.adjust_settings_entry(-1),
                    NavAction::Right => self.adjust_settings_entry(1),
                    NavAction::Confirm => self.activate_settings_entry(ctx),
                    NavAction::PrevTab | NavAction::NextTab => {}
                    NavAction::Cancel => self.settings_pane = SettingsPane::Sidebar,
                }
            }
        }
    }

    fn adjust_settings_entry(&mut self, delta: i32) {
        let entries = self.settings_entries();
        let Some(entry) = entries.get(self.settings_content_focus) else {
            return;
        };
        match entry {
            SettingsEntry::Option(key) => self.cycle_option_value(key, delta),
            SettingsEntry::ShaderPreset => self.cycle_shader_preset(delta),
            SettingsEntry::ShaderSlider(control) => self.adjust_shader_slider(*control, delta),
            _ => {}
        }
    }

    fn activate_settings_entry(&mut self, ctx: &egui::Context) {
        let entries = self.settings_entries();
        let Some(entry) = entries.get(self.settings_content_focus).cloned() else {
            return;
        };
        match entry {
            SettingsEntry::ResetAll => self.reset_all_options(),
            SettingsEntry::RestartCore => {
                self.worker.send(Command::RestartCore);
                self.push_toast("Restarting core…".into(), MessageKind::Info, 2.0);
            }
            SettingsEntry::ReloadThemes => self.reload_theme_catalog(ctx),
            SettingsEntry::Theme(name) => self.apply_theme_name(&name, ctx),
            SettingsEntry::Option(key) => self.cycle_option_value(&key, 1),
            SettingsEntry::OptionReset(key) => self.reset_option_value(&key),
            SettingsEntry::ShaderPreset => self.cycle_shader_preset(1),
            SettingsEntry::ShaderSlider(_) => {}
            SettingsEntry::ShaderReset => self.reset_shader_defaults(),
            SettingsEntry::StretchToFill => {
                self.config.stretch_to_fill = !self.config.stretch_to_fill;
                let _ = self.config.save();
            }
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(info) = self.core_info.as_ref() else {
            return;
        };
        let host = self.host.clone();
        let shader = self.shader.clone();
        let mut shader_dirty = false;
        let mut changes: Vec<(String, String)> = Vec::new();
        let mut resets: Vec<String> = Vec::new();
        let mut reset_all = false;
        let mut restart_core = false;

        let visible = host.options.read().unwrap().visible.clone();
        let overrides = host.options.read().unwrap().overrides.clone();
        let categories = self.settings_categories();
        let category_info: std::collections::HashMap<String, String> = info
            .categories
            .iter()
            .map(|category| (category.key.clone(), category.info.clone()))
            .collect();
        let settings_pane = self.settings_pane;
        let sidebar_focus = self.settings_sidebar_focus;
        let content_focus = self.settings_content_focus;
        let options = info.options.clone();
        let themes = self.themes.clone();
        let current_theme = self.config.theme.clone();
        let active_accent = self.ui_theme.accent;
        let theme_error = self.theme_error.clone();
        let mut theme_change: Option<String> = None;
        let mut reload_themes = false;
        let mut back = false;

        egui::CentralPanel::default().show(ui, |ui| {
            if theme::page_header(ui, "Settings", &self.ui_theme) {
                back = true;
            }
            theme::window_body(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.settings_category == "video" {
                        ui.label("Game shader");
                        ui.weak("Applied live to the game view only.");
                    } else {
                        ui.label("Search");
                        let hint = if self.settings_category == "appearance" {
                            "filter themes"
                        } else {
                            "filter options"
                        };
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings_search)
                                .desired_width(320.0)
                                .hint_text(hint),
                        );
                        if self.settings_category == "appearance" {
                            let focused = settings_pane == SettingsPane::Content
                                && content_focus == 0;
                            let response = ui.button("Reload themes");
                            focus_ring(ui, response.rect, focused, &self.ui_theme);
                            if response.clicked() {
                                reload_themes = true;
                                self.settings_content_focus = 0;
                            }
                        } else if self.settings_category != "video" {
                            let reset_focused = settings_pane == SettingsPane::Content
                                && content_focus == 0;
                            let response = ui.button("Reset all");
                            focus_ring(ui, response.rect, reset_focused, &self.ui_theme);
                            if response.clicked() {
                                reset_all = true;
                                self.settings_content_focus = 0;
                            }
                            let restart_focused = settings_pane == SettingsPane::Content
                                && content_focus == 1;
                            let response = ui.button("Restart core");
                            focus_ring(ui, response.rect, restart_focused, &self.ui_theme);
                            if response.clicked() {
                                restart_core = true;
                                self.settings_content_focus = 1;
                            }
                            let selected = self
                                .host
                                .option_value("swanstation_GPU_Renderer")
                                .unwrap_or_else(|| "Auto".to_string());
                            let active = match self.host.renderer_decision() {
                                Some(RendererDecision::OpenGl) if self.host.hardware_active() => {
                                    "OpenGL (hardware)"
                                }
                                Some(RendererDecision::OpenGl) => {
                                    "OpenGL ready (waiting for frame)"
                                }
                                Some(RendererDecision::Software) if selected == "Software" => {
                                    "Software"
                                }
                                Some(RendererDecision::Software) => {
                                    "Software (OpenGL unavailable)"
                                }
                                None if selected == "Software" => "Software",
                                None => "renderer not negotiated yet",
                            };
                            ui.weak(format!("Renderer: {active} · selected {selected}"));
                        }
                    }
                });
                if self.settings_category == "appearance" {
                    ui.weak(format!(
                        "External themes are appended from {}",
                        theme::external_path().display()
                    ));
                    if let Some(error) = &theme_error {
                        ui.colored_label(self.ui_theme.error, error);
                    }
                } else if self.settings_category == "video" {
                    ui.weak(
                        "CRT/bloom post-processing sits on top of the game image. \
                         Presets are inspired approximations, not full CRT Royale or Blurpi shader packs.",
                    );
                } else {
                    ui.weak(
                        "Most options apply on the next frame. Use Restart core to re-read all options; \
                         options marked \"requires restart\" need it.",
                    );
                }
                ui.separator();

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(230.0);
                        for (index, (key, label)) in categories.iter().enumerate() {
                            let focused =
                                settings_pane == SettingsPane::Sidebar
                                    && sidebar_focus == index;
                            let mut response =
                                ui.selectable_label(self.settings_category == *key, label);
                            if let Some(info) = category_info.get(key) {
                                if !info.is_empty() {
                                    response = response.on_hover_text(info);
                                }
                            }
                            focus_ring(ui, response.rect, focused, &self.ui_theme);
                            if response.clicked() {
                                self.settings_category = key.clone();
                                self.settings_sidebar_focus = index;
                                self.settings_content_focus = 0;
                                self.settings_pane = SettingsPane::Content;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(850.0);
                        if self.settings_category == "appearance" {
                            ui.heading("Theme");
                            ui.weak(
                                "Select a theme for the launcher, menus, settings and splash.",
                            );
                            ui.label(
                                egui::RichText::new(format!("Current theme: {current_theme}"))
                                    .color(self.ui_theme.muted),
                            );
                            ui.add_space(6.0);
                            let search = self.settings_search.to_lowercase();
                            let mut shown = 0;
                            let mut entry_index = 1;
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .max_height((ui.available_height() - 16.0).max(200.0))
                                .show(ui, |ui| {
                                    for theme in &themes {
                                        if !search.is_empty()
                                            && !theme.name.to_lowercase().contains(&search)
                                        {
                                            continue;
                                        }
                                        shown += 1;
                                        let focused = settings_pane == SettingsPane::Content
                                            && content_focus == entry_index;
                                        let response = theme::theme_picker_row(
                                            ui,
                                            theme,
                                            theme.name == current_theme,
                                            active_accent,
                                        );
                                        focus_ring(ui, response.rect, focused, &self.ui_theme);
                                        if response.clicked() {
                                            theme_change = Some(theme.name.clone());
                                            self.settings_content_focus = entry_index;
                                        }
                                        if focused {
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        entry_index += 1;
                                        ui.add_space(6.0);
                                    }
                                    if shown == 0 {
                                        ui.weak("No matching themes.");
                                    }
                                });
                        } else if self.settings_category == "video" {
                            ui.heading("Game shader");
                            ui.weak(
                                "Applied to the game image only. Menus, FPS text and overlays stay sharp.",
                            );
                            ui.add_space(8.0);

                            let current_preset =
                                ShaderPreset::parse(&self.config.shader_preset);
                            let focused = settings_pane == SettingsPane::Content
                                && content_focus == 0;
                            let combo = egui::ComboBox::from_id_salt("shader-preset")
                                .width(300.0)
                                .selected_text(current_preset.label())
                                .show_ui(ui, |ui| {
                                    for preset in ShaderPreset::ALL {
                                        if ui
                                            .selectable_label(
                                                current_preset == preset,
                                                preset.label(),
                                            )
                                            .clicked()
                                        {
                                            self.apply_shader_preset(preset);
                                            shader_dirty = true;
                                        }
                                    }
                                });
                            focus_ring(ui, combo.response.rect, focused, &self.ui_theme);
                            if focused {
                                combo.response.scroll_to_me(Some(egui::Align::Center));
                            }
                            ui.add_space(8.0);

                            let mut changed = false;
                            let response = shader_slider(
                                ui,
                                &self.ui_theme,
                                &mut self.config.shader_bloom,
                                0.0..=2.0,
                                "Bloom strength",
                            );
                            focus_ring(
                                ui,
                                response.rect,
                                settings_pane == SettingsPane::Content && content_focus == 1,
                                &self.ui_theme,
                            );
                            changed |= response.changed();
                            let response = shader_slider(
                                ui,
                                &self.ui_theme,
                                &mut self.config.shader_scanlines,
                                0.0..=1.0,
                                "Scanline strength",
                            );
                            focus_ring(
                                ui,
                                response.rect,
                                settings_pane == SettingsPane::Content && content_focus == 2,
                                &self.ui_theme,
                            );
                            changed |= response.changed();
                            let response = shader_slider(
                                ui,
                                &self.ui_theme,
                                &mut self.config.shader_curvature,
                                0.0..=0.25,
                                "Curvature",
                            );
                            focus_ring(
                                ui,
                                response.rect,
                                settings_pane == SettingsPane::Content && content_focus == 3,
                                &self.ui_theme,
                            );
                            changed |= response.changed();
                            let response = shader_slider(
                                ui,
                                &self.ui_theme,
                                &mut self.config.shader_mask,
                                0.0..=1.0,
                                "Aperture mask",
                            );
                            focus_ring(
                                ui,
                                response.rect,
                                settings_pane == SettingsPane::Content && content_focus == 4,
                                &self.ui_theme,
                            );
                            changed |= response.changed();
                            let response = shader_slider(
                                ui,
                                &self.ui_theme,
                                &mut self.config.shader_vignette,
                                0.0..=1.0,
                                "Vignette",
                            );
                            focus_ring(
                                ui,
                                response.rect,
                                settings_pane == SettingsPane::Content && content_focus == 5,
                                &self.ui_theme,
                            );
                            changed |= response.changed();
                            if changed {
                                shader.lock().unwrap().clear_error();
                                shader_dirty = true;
                            }

                            ui.add_space(8.0);
                            let focused = settings_pane == SettingsPane::Content
                                && content_focus == 6;
                            let response = ui.button("Reset to preset defaults");
                            focus_ring(ui, response.rect, focused, &self.ui_theme);
                            if focused {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                self.reset_shader_defaults();
                                shader_dirty = true;
                            }

                            if let Some(error) = shader.lock().unwrap().error() {
                                ui.add_space(6.0);
                                ui.colored_label(
                                    self.ui_theme.error,
                                    format!("Shader unavailable: {error}"),
                                );
                                ui.weak("Falling back to the unshaded game image.");
                            }

                            ui.add_space(10.0);
                            ui.weak(
                                "Soft CRT, Blurpi-inspired and Royale-inspired are built-in \
                                 approximations. They are not ports of the full external shader packs.",
                            );
                        } else {
                            egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height((ui.available_height() - 16.0).max(200.0))
                            .show(ui, |ui| {
                                let search = self.settings_search.to_lowercase();
                                let mut shown = 0;
                                let mut entry_index = 2;
                                if self.settings_category == "display" {
                                    let focused = settings_pane == SettingsPane::Content
                                        && content_focus == 2;
                                    let response = ui.checkbox(
                                        &mut self.config.stretch_to_fill,
                                        "Stretch game to fill screen",
                                    );
                                    focus_ring(ui, response.rect, focused, &self.ui_theme);
                                    if focused {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if response.changed() {
                                        let _ = self.config.save();
                                    }
                                    if response.clicked() {
                                        self.settings_content_focus = 2;
                                    }
                                    ui.weak(
                                        "Fills the game view with no borders; geometry may be stretched.",
                                    );
                                    ui.separator();
                                    entry_index = 3;
                                }
                                egui::Grid::new("settings-options-grid")
                                    .num_columns(2)
                                    .striped(true)
                                    .spacing([16.0, 8.0])
                                    .show(ui, |ui| {
                                        for option in &options {
                                            if option.key == "swanstation_GPU_ResolutionScale" {
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
                                                        || !categories.iter().any(|(key, _)| {
                                                            key == &option.category_key
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
                                            let focused = settings_pane
                                                == SettingsPane::Content
                                                && content_focus == entry_index;
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
                                                || option.info.contains("restart")
                                                || option.key == "swanstation_GPU_Renderer";
                                            ui.vertical(|ui| {
                                                ui.set_min_width(380.0);
                                                ui.label(
                                                    egui::RichText::new(&option.label)
                                                        .strong()
                                                        .color(self.ui_theme.text),
                                                );
                                                if restart {
                                                    ui.small(
                                                        egui::RichText::new("requires restart")
                                                            .color(self.ui_theme.warning),
                                                    );
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                let response = egui::ComboBox::from_id_salt(
                                                    format!("opt-{}", option.key),
                                                )
                                                .width(300.0)
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
                                                focus_ring(
                                                    ui,
                                                    response.response.rect,
                                                    focused,
                                                    &self.ui_theme,
                                                );
                                                if response.response.clicked() {
                                                    self.settings_content_focus = entry_index;
                                                }
                                                if focused {
                                                    response
                                                        .response
                                                        .scroll_to_me(Some(egui::Align::Center));
                                                }
                                                entry_index += 1;
                                                if overrides.contains_key(&option.key) {
                                                    let reset_focused = settings_pane
                                                        == SettingsPane::Content
                                                        && content_focus == entry_index;
                                                    let reset_response =
                                                        ui.small_button("reset");
                                                    focus_ring(
                                                        ui,
                                                        reset_response.rect,
                                                        reset_focused,
                                                        &self.ui_theme,
                                                    );
                                                    if reset_response.clicked() {
                                                        resets.push(option.key.clone());
                                                        self.settings_content_focus =
                                                            entry_index;
                                                    }
                                                    if reset_focused {
                                                        reset_response.scroll_to_me(Some(
                                                            egui::Align::Center,
                                                        ));
                                                    }
                                                    entry_index += 1;
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
                        }
                    });
                });
                });
            });
        if back {
            self.back_page();
        }

        if reload_themes {
            self.reload_theme_catalog(&ctx);
        }
        if let Some(name) = theme_change {
            self.apply_theme_name(&name, &ctx);
        }

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
        if shader_dirty {
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

    fn apply_theme_name(&mut self, name: &str, ctx: &egui::Context) {
        let Some(theme) = self.themes.iter().find(|theme| theme.name == name).cloned() else {
            return;
        };
        self.ui_theme = theme.apply(ctx);
        self.config.theme = theme.name.clone();
        let _ = self.config.save();
        self.push_toast(format!("Theme: {}", theme.name), MessageKind::Info, 1.5);
    }

    fn reload_theme_catalog(&mut self, ctx: &egui::Context) {
        let (themes, error) = theme::load_catalog();
        self.themes = themes;
        self.theme_error = error;
        if self.themes.is_empty() {
            self.themes.push(Theme::fallback());
        }
        let index = self
            .themes
            .iter()
            .position(|theme| theme.name == self.config.theme)
            .unwrap_or(0);
        let name = self.themes[index].name.clone();
        if name != self.config.theme {
            self.config.theme = name.clone();
            let _ = self.config.save();
        }
        self.apply_theme_name(&name, ctx);
        if let Some(error) = &self.theme_error {
            self.push_toast(error.clone(), MessageKind::Warning, 6.0);
        } else {
            self.push_toast("Themes reloaded".into(), MessageKind::Info, 2.0);
        }
    }

    fn controller_page(&mut self, ui: &mut egui::Ui) {
        let mut dirty = false;
        let mut device_change: Option<(u32, u32)> = None;
        let mut rescan = false;
        let info = self.core_info.clone();
        let host = self.host.clone();

        let ui_theme = self.ui_theme;
        let mut back = false;
        let sdl = &mut self.sdl;
        let config = &mut self.config;

        egui::CentralPanel::default().show(ui, |ui| {
            if theme::page_header(ui, "Controller", &ui_theme) {
                back = true;
            }
            theme::window_body(ui, |ui| {
                    ui.heading("SDL game controllers");
                    let controllers = sdl.controllers().to_vec();

                    ui.horizontal(|ui| {
                        ui.label("Player 1");
                        egui::ComboBox::from_id_salt("controller-window-p1")
                            .width(380.0)
                            .selected_text(player_display_name(sdl, 0))
                            .show_ui(ui, |ui| {
                                if controllers.is_empty() {
                                    ui.weak("No SDL game controllers found");
                                }
                                let other = sdl.player_instance(1);
                                for controller in &controllers {
                                    if Some(controller.instance_id) == other {
                                        continue;
                                    }
                                    let label = controller_label(&controllers, controller);
                                    if ui
                                        .selectable_label(
                                            sdl.player_instance(0)
                                                == Some(controller.instance_id),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        sdl.set_player_instance(
                                            0,
                                            Some(controller.instance_id),
                                        );
                                        config.controller_guid = controller.guid.clone();
                                        dirty = true;
                                    }
                                }
                            });
                    });

                    ui.horizontal(|ui| {
                        ui.label("Player 2");
                        egui::ComboBox::from_id_salt("controller-window-p2")
                            .width(380.0)
                            .selected_text(player_display_name(sdl, 1))
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        sdl.player_guid(1).is_empty()
                                            && sdl.player_instance(1).is_none(),
                                        "None",
                                    )
                                    .clicked()
                                {
                                    sdl.set_player_instance(1, None);
                                    config.controller_guid_p2.clear();
                                    dirty = true;
                                }
                                let other = sdl.player_instance(0);
                                for controller in &controllers {
                                    if Some(controller.instance_id) == other {
                                        continue;
                                    }
                                    let label = controller_label(&controllers, controller);
                                    if ui
                                        .selectable_label(
                                            sdl.player_instance(1)
                                                == Some(controller.instance_id),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        sdl.set_player_instance(
                                            1,
                                            Some(controller.instance_id),
                                        );
                                        config.controller_guid_p2 = controller.guid.clone();
                                        dirty = true;
                                    }
                                }
                            });
                    });

                    let response = ui.button("Rescan controllers");
                    if response.clicked() {
                        rescan = true;
                    }
                    ui.weak("Player 1 drives the launcher and menus. Player 2 is game-only. PS / Guide opens the in-game menu.");
                    ui.separator();

                    if let Some(info) = &info {
                        for port in 0..PORTS {
                            let Some(port_info) = info.ports.get(port) else {
                                continue;
                            };
                            let current_id = if port == 0 {
                                config.port1_device
                            } else {
                                config.port2_device
                            }
                            .or_else(|| port_info.types.first().map(|(_, id)| *id));
                            let current_name = current_id
                                .and_then(|id| {
                                    port_info
                                        .types
                                        .iter()
                                        .find(|(_, i)| *i == id)
                                        .map(|(n, _)| n.clone())
                                })
                                .unwrap_or_else(|| "Unknown".to_string());
                            ui.horizontal(|ui| {
                                ui.label(if port == 0 {
                                    "Port 1 (Player 1)"
                                } else {
                                    "Port 2 (Player 2)"
                                });
                                egui::ComboBox::from_id_salt(format!(
                                    "device-type-{port}"
                                ))
                                .width(300.0)
                                .selected_text(current_name)
                                .show_ui(ui, |ui| {
                                    for (name, id) in &port_info.types {
                                        if ui
                                            .selectable_label(
                                                Some(*id) == current_id,
                                                name,
                                            )
                                            .clicked()
                                        {
                                            if port == 0 {
                                                config.port1_device = Some(*id);
                                            } else {
                                                config.port2_device = Some(*id);
                                            }
                                            device_change = Some((port as u32, *id));
                                            dirty = true;
                                        }
                                    }
                                });
                            });
                        }
                        if info.ports.len() < PORTS {
                            ui.weak("This core exposes only one controller port.");
                        }
                    }
                    ui.add(
                        egui::Slider::new(&mut config.input.deadzone, 0.0..=0.6)
                            .text("Stick deadzone"),
                    );
                    let response = ui.checkbox(
                        &mut config.input.pause_on_menu,
                        "Pause game while the menu is open",
                    );
                    if response.changed() {
                        dirty = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label("Fallback menu combo:");
                        let response = ui.selectable_label(
                            config.input.menu_button == "guide",
                            "None (PS/Guide only)",
                        );
                        if response.clicked() {
                            config.input.menu_button = "guide".into();
                            dirty = true;
                        }
                        let response = ui.selectable_label(
                            config.input.menu_button == "select_start",
                            "Select + Start",
                        );
                        if response.clicked() {
                            config.input.menu_button = "select_start".into();
                            dirty = true;
                        }
                    });
                    ui.separator();
                    ui.label("Live input");
                    for player in 0..PORTS {
                        ui.monospace(format!(
                            "Player {} buttons: {:016b}   axes: {} {} {} {}",
                            player + 1,
                            host.input.buttons(player) & 0xffff,
                            host.input.axis(player, 0, 0),
                            host.input.axis(player, 0, 1),
                            host.input.axis(player, 1, 0),
                            host.input.axis(player, 1, 1),
                        ));
                        ui.monospace(format!(
                            "Player {} last button: {}",
                            player + 1,
                            sdl.last_button(player)
                        ));
                    }
                });
            });
        if back {
            self.back_page();
        }
        if rescan {
            self.sdl.refresh_controllers();
            dirty = true;
        }
        if let Some((port, id)) = device_change {
            self.port_devices[port as usize] = Some(id);
            let enabled = port == 0 || self.sdl.player_instance(1).is_some();
            if enabled {
                self.worker.send(Command::SetPortDevice(port, id));
                if port == 1 {
                    self.p2_port_active = true;
                }
            }
            dirty = true;
        }
        if dirty {
            self.config.controller_guid = self.sdl.player_guid(0).to_string();
            self.config.controller_guid_p2 = self.sdl.player_guid(1).to_string();
            self.ensure_player2_device();
            let _ = self.config.save();
        }
        self.sync_player2_port();
    }

    fn memory_card_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ui_theme = self.ui_theme;
        let (back, toast) = self.memory_card.page(ui, &ctx, &ui_theme);
        if back {
            self.back_page();
        }
        if self.memory_card.source_dir_dirty {
            self.config.memory_card_source_dir = self.memory_card.source_dir.clone();
            let _ = self.config.save();
            self.memory_card.source_dir_dirty = false;
        }
        if let Some((text, kind)) = toast {
            self.push_toast(text, kind, 4.0);
        }
    }

    fn browser_window(&mut self, ctx: &egui::Context) {
        let mut result = BrowseResult::None;
        let kind = self.file_browser.as_ref().map(|b| b.kind);
        let ui_theme = self.ui_theme;
        if let Some(browser) = self.file_browser.as_mut() {
            result = browser.show(ctx, &ui_theme);
        }
        if matches!(result, BrowseResult::None) {
            return;
        }
        self.file_browser = None;
        if let Some(kind) = kind {
            self.apply_browse_result(result, kind);
        }
    }

    fn apply_browse_result(&mut self, result: BrowseResult, kind: BrowseKind) {
        let BrowseResult::Chosen(path) = result else {
            return;
        };
        match kind {
            BrowseKind::Game => {
                self.config.last_game = path.to_string_lossy().to_string();
                let _ = self.config.save();
                // Put the launcher focus back on Play so the next Cross starts
                // the newly selected game.
                self.launcher_focus = 0;
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
            BrowseKind::MemoryCard => {
                // The Memory Card Manager owns its own file browser.
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
                ui.set_max_width(560.0);
                for toast in &self.toasts {
                    let color = match toast.kind {
                        MessageKind::Info => self.ui_theme.info,
                        MessageKind::Warning => self.ui_theme.warning,
                        MessageKind::Error => self.ui_theme.error,
                        MessageKind::Debug => self.ui_theme.muted,
                    };
                    egui::Frame::NONE
                        .fill(color)
                        .inner_margin(egui::Margin::same(8))
                        .corner_radius(egui::CornerRadius::same(5))
                        .show(ui, |ui| {
                            let text_color = self.ui_theme.on_color(color);
                            ui.label(egui::RichText::new(&toast.text).color(text_color));
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
        self.poll_clipboard();
        self.poll_sdl(&ctx);
        self.update_measured_fps();
        let splash_finished = self.handle_splash(&ctx);
        if splash_finished {
            // Consume the key/click/controller press that dismissed the splash
            // instead of letting it activate a launcher row in the same frame.
            self.splash(ui);
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        if !self.ui_input_locked(&ctx) {
            self.handle_frontend_keys(&ctx);
            self.handle_hotkeys(&ctx);
        }

        match self.screen {
            Screen::Splash => self.splash(ui),
            Screen::Launcher => self.launcher(ui),
            Screen::Game => self.game(ui),
            Screen::Settings => self.settings_page(ui),
            Screen::Controller => self.controller_page(ui),
            Screen::MemoryCard => self.memory_card_page(ui),
        }

        self.browser_window(&ctx);
        if self.screen != Screen::Splash {
            self.draw_toasts(&ctx);
        }

        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.ui_theme.background.to_normalized_gamma_f32()
    }
}

fn fast_forward_enabled(latched: bool, tab_held: bool, overlay_open: bool, typing: bool) -> bool {
    !overlay_open && !typing && (latched || tab_held)
}

fn game_image_rect(view: egui::Rect, frame: [usize; 2], stretch: bool) -> egui::Rect {
    if stretch {
        return view;
    }
    let w = frame[0].max(1) as f32;
    let h = frame[1].max(1) as f32;
    let scale = (view.width() / w).min(view.height() / h).max(0.01);
    egui::Rect::from_center_size(view.center(), egui::vec2(w * scale, h * scale))
}

fn focus_ring(ui: &egui::Ui, rect: egui::Rect, focused: bool, theme: &UiTheme) {
    if !focused {
        return;
    }
    ui.painter().rect_stroke(
        rect.expand(2.0),
        egui::CornerRadius::same(7),
        egui::Stroke::new(2.0, theme.accent),
        egui::StrokeKind::Outside,
    );
}

/// Sliders need their own rail/handle colours: the regular inactive widget
/// fill matches the window background in several themes, which hides the
/// track entirely. Keep the accent trailing fill so the current value stays
/// readable in both dark and light themes.
fn shader_slider(
    ui: &mut egui::Ui,
    theme: &UiTheme,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    text: &str,
) -> egui::Response {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.slider_trailing_fill = true;
        visuals.widgets.inactive.bg_fill = theme.focus_fill;
        visuals.widgets.inactive.weak_bg_fill = theme.focus_fill;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.5, theme.accent);
        ui.add(egui::Slider::new(value, range).text(text))
    })
    .inner
}

fn settings_category_label<'a>(key: &'a str, fallback: &'a str) -> &'a str {
    match key {
        "console" => "Console",
        "enhancement" => "Graphics",
        "display" => "Display",
        "port" => "Ports",
        "advanced" => "Advanced",
        _ if fallback.is_empty() => key,
        _ => fallback,
    }
}

fn choose_port_device(types: &[(String, u32)], configured: Option<u32>) -> Option<u32> {
    let valid = |id: u32| types.iter().any(|(_, i)| *i == id);
    configured
        .filter(|id| valid(*id))
        .or_else(|| {
            types
                .iter()
                .find(|(name, _)| name.contains("DualShock") || name.contains("Analog"))
                .map(|(_, id)| *id)
        })
        .or_else(|| types.first().map(|(_, id)| *id))
}

fn player_display_name(sdl: &SdlState, player: usize) -> String {
    sdl.player_name(player)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if sdl.player_guid(player).is_empty() {
                "None".to_string()
            } else {
                "Disconnected".to_string()
            }
        })
}

fn controller_label(controllers: &[ControllerEntry], entry: &ControllerEntry) -> String {
    let duplicates = controllers
        .iter()
        .filter(|controller| controller.guid == entry.guid)
        .count();
    if duplicates > 1 {
        format!("{} [{}]", entry.name, entry.instance_id)
    } else {
        entry.name.clone()
    }
}

fn consume_escape(ctx: &egui::Context) {
    ctx.input_mut(|input| {
        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
    });
}

fn key_to_nav(ctx: &egui::Context) -> Option<NavAction> {
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
        Some(NavAction::Up)
    } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
        Some(NavAction::Down)
    } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
        Some(NavAction::Left)
    } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
        Some(NavAction::Right)
    } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        Some(NavAction::Confirm)
    } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        Some(NavAction::Cancel)
    } else if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
        Some(NavAction::PrevTab)
    } else if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
        Some(NavAction::NextTab)
    } else {
        None
    }
}

fn short_game_name(path: &str) -> String {
    let path = Path::new(path);
    let name = path
        .file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    if name.chars().count() > 70 {
        let mut out: String = name.chars().take(70).collect();
        out.push('…');
        out
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> Vec<(String, u32)> {
        vec![
            ("Digital Controller".to_string(), 1),
            ("DualShock Controller".to_string(), 258),
            ("Analog Controller".to_string(), 261),
        ]
    }

    #[test]
    fn configured_port_device_wins_when_valid() {
        assert_eq!(choose_port_device(&types(), Some(1)), Some(1));
    }

    #[test]
    fn invalid_port_device_falls_back_to_analog() {
        assert_eq!(choose_port_device(&types(), Some(999)), Some(258));
    }

    #[test]
    fn port_device_falls_back_to_first_type() {
        let digital_only = vec![("Digital Controller".to_string(), 1)];
        assert_eq!(choose_port_device(&digital_only, None), Some(1));
    }

    #[test]
    fn empty_port_types_have_no_device() {
        assert_eq!(choose_port_device(&[], Some(258)), None);
    }

    #[test]
    fn fps_meter_waits_for_first_sample() {
        let mut meter = FpsMeter::default();
        assert_eq!(meter.display(), "-- FPS");
        meter.update(0, Instant::now());
        assert_eq!(meter.display(), "-- FPS");
    }

    #[test]
    fn fps_meter_reports_measured_rate() {
        let base = Instant::now();
        let mut meter = FpsMeter::default();
        meter.update(0, base);
        meter.update(30, base + Duration::from_millis(500));
        assert!((meter.value() - 60.0).abs() < 0.01);
        assert_eq!(meter.display(), "60.0 FPS");
    }

    #[test]
    fn fps_meter_suspend_keeps_last_value() {
        let base = Instant::now();
        let mut meter = FpsMeter::default();
        meter.update(0, base);
        meter.update(30, base + Duration::from_millis(500));
        let before = meter.display();
        meter.suspend();
        assert_eq!(meter.display(), before);
    }

    #[test]
    fn fps_meter_zero_frames_reports_zero() {
        let base = Instant::now();
        let mut meter = FpsMeter::default();
        meter.update(100, base);
        meter.update(100, base + Duration::from_millis(500));
        assert_eq!(meter.display(), "0.0 FPS");
    }

    #[test]
    fn duplicate_guid_controllers_show_instance_ids() {
        let controllers = vec![
            ControllerEntry {
                index: 0,
                instance_id: 10,
                name: "DualShock 4".to_string(),
                guid: "same".to_string(),
            },
            ControllerEntry {
                index: 1,
                instance_id: 11,
                name: "DualShock 4".to_string(),
                guid: "same".to_string(),
            },
        ];
        assert!(controller_label(&controllers, &controllers[0]).contains("[10]"));
        assert!(controller_label(&controllers, &controllers[1]).contains("[11]"));
    }

    #[test]
    fn settings_categories_use_short_labels() {
        assert_eq!(
            settings_category_label("console", "Console Settings"),
            "Console"
        );
        assert_eq!(
            settings_category_label("enhancement", "Enhancement Settings"),
            "Graphics"
        );
        assert_eq!(
            settings_category_label("display", "Display Settings"),
            "Display"
        );
        assert_eq!(settings_category_label("port", "Port Settings"), "Ports");
        assert_eq!(
            settings_category_label("advanced", "Advanced Settings"),
            "Advanced"
        );
        assert_eq!(
            settings_category_label("custom", "Custom Category"),
            "Custom Category"
        );
    }

    #[test]
    fn game_image_rect_fits_or_stretches() {
        let view = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1000.0, 1000.0));
        let fit = game_image_rect(view, [320, 240], false);
        assert_eq!(fit.width(), 1000.0);
        assert_eq!(fit.height(), 750.0);
        assert_eq!(fit.center(), view.center());

        let stretch = game_image_rect(view, [320, 240], true);
        assert_eq!(stretch, view);
    }

    #[test]
    fn fast_forward_combines_latch_tab_and_menu_state() {
        assert!(!fast_forward_enabled(false, false, false, false));
        assert!(fast_forward_enabled(true, false, false, false));
        assert!(fast_forward_enabled(false, true, false, false));
        assert!(!fast_forward_enabled(true, true, true, false));
        assert!(!fast_forward_enabled(true, true, false, true));
    }
}
