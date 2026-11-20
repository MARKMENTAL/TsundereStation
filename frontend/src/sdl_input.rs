use crate::config::Config;
use crate::libretro::audio::AudioRing;
use crate::libretro::ffi;
use crate::libretro::host::{HostState, MessageKind};
use crate::libretro::input::PORTS;
use crate::splash::SplashAudio;
use anyhow::{anyhow, Result};
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::GameControllerSubsystem;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ControllerEntry {
    pub index: u32,
    pub instance_id: u32,
    pub name: String,
    pub guid: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavAction {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Cancel,
}

struct PlayerSlot {
    guid: String,
    instance: Option<u32>,
    controller: Option<GameController>,
}

impl PlayerSlot {
    fn new(guid: &str) -> Self {
        Self {
            guid: guid.to_string(),
            instance: None,
            controller: None,
        }
    }
}

pub struct RingAudio {
    ring: Arc<AudioRing>,
    splash: Arc<Mutex<SplashAudio>>,
}

impl AudioCallback for RingAudio {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        self.ring.fill(out);
        if let Ok(mut splash) = self.splash.lock() {
            for sample in out.iter_mut() {
                *sample = (*sample + splash.next_sample()).clamp(-1.0, 1.0);
            }
        }
    }
}

pub struct SdlState {
    event_pump: sdl2::EventPump,
    game_controllers: GameControllerSubsystem,
    controllers: Vec<ControllerEntry>,
    players: [PlayerSlot; PORTS],
    combo_was_down: bool,
    last_menu_toggle: Option<Instant>,
    last_rumble: [Instant; PORTS],
    last_rumble_values: [(u16, u16); PORTS],
    last_button: [String; PORTS],
    activity: bool,
    suppress_input: bool,
    suppress_deadline: Option<Instant>,
    nav_prev: [bool; 6],
    nav_repeat_at: Option<Instant>,
    // Keep the subsystem/device handles alive and drop them before SDL_Quit.
    _audio_device: Option<AudioDevice<RingAudio>>,
    _audio_subsystem: Option<sdl2::AudioSubsystem>,
    _sdl: sdl2::Sdl,
}

impl SdlState {
    pub fn new(
        ring: Arc<AudioRing>,
        preferred_guid: &str,
        preferred_guid_p2: &str,
        splash: Arc<Mutex<SplashAudio>>,
    ) -> Result<Self> {
        let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "1");
        let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS4", "1");
        let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS5", "1");

        let sdl = sdl2::init().map_err(|e| anyhow!("SDL init failed: {e}"))?;
        let game_controllers = sdl
            .game_controller()
            .map_err(|e| anyhow!("SDL game controller init failed: {e}"))?;
        let event_pump = sdl
            .event_pump()
            .map_err(|e| anyhow!("SDL event pump failed: {e}"))?;

        let (audio_subsystem, audio_device) = match sdl.audio() {
            Ok(subsystem) => {
                let desired = AudioSpecDesired {
                    freq: Some(44100),
                    channels: Some(2),
                    samples: Some(1024),
                };
                let ring_for_device = ring.clone();
                let splash_for_device = splash.clone();
                match subsystem.open_playback(None, &desired, move |_spec| RingAudio {
                    ring: ring_for_device,
                    splash: splash_for_device,
                }) {
                    Ok(device) => {
                        let spec = device.spec();
                        if spec.freq != 44100 || spec.channels != 2 {
                            log::warn!(
                                "SDL audio opened at {} Hz / {} channels; emulator audio is 44100 Hz stereo",
                                spec.freq,
                                spec.channels
                            );
                        }
                        device.resume();
                        ring.set_enabled(true);
                        (Some(subsystem), Some(device))
                    }
                    Err(e) => {
                        log::warn!("Could not open SDL audio device: {e}");
                        (Some(subsystem), None)
                    }
                }
            }
            Err(e) => {
                log::warn!("Could not initialize SDL audio: {e}");
                (None, None)
            }
        };

        let now = Instant::now();
        let mut state = Self {
            _sdl: sdl,
            _audio_subsystem: audio_subsystem,
            _audio_device: audio_device,
            event_pump,
            game_controllers,
            controllers: Vec::new(),
            players: [
                PlayerSlot::new(preferred_guid),
                PlayerSlot::new(preferred_guid_p2),
            ],
            combo_was_down: false,
            last_menu_toggle: None,
            last_rumble: [now; PORTS],
            last_rumble_values: [(0, 0); PORTS],
            last_button: std::array::from_fn(|_| String::new()),
            activity: false,
            suppress_input: false,
            suppress_deadline: None,
            nav_prev: [false; 6],
            nav_repeat_at: None,
        };
        state.refresh_controllers();
        Ok(state)
    }

    pub fn controllers(&self) -> &[ControllerEntry] {
        &self.controllers
    }

    pub fn player_guid(&self, player: usize) -> &str {
        self.players
            .get(player)
            .map(|slot| slot.guid.as_str())
            .unwrap_or("")
    }

    pub fn player_instance(&self, player: usize) -> Option<u32> {
        self.players.get(player).and_then(|slot| slot.instance)
    }

    pub fn player_name(&self, player: usize) -> Option<&str> {
        let instance = self.player_instance(player)?;
        self.controllers
            .iter()
            .find(|controller| controller.instance_id == instance)
            .map(|controller| controller.name.as_str())
    }

    pub fn last_button(&self, player: usize) -> &str {
        self.last_button
            .get(player)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Any controller button activity since the last call, used to skip the
    /// splash screen.
    pub fn take_activity(&mut self) -> bool {
        std::mem::take(&mut self.activity)
    }

    /// Discard gamepad input after resuming from the menu. Input stays zeroed
    /// until the buttons/sticks return to neutral, or `max` elapses as a
    /// stuck-controller safety valve.
    pub fn suppress_input_until_release(&mut self, max: Duration) {
        self.suppress_input = true;
        self.suppress_deadline = Some(Instant::now() + max);
        log::info!(
            "Suppressing gamepad input until neutral (max {} ms)",
            max.as_millis()
        );
    }

    pub fn clear_input_lockout(&mut self) {
        if self.suppress_input {
            log::debug!("Gamepad input lockout cleared");
        }
        self.suppress_input = false;
        self.suppress_deadline = None;
    }

    /// Bind a specific controller instance to a player. Passing `None` clears
    /// the assignment. The GUID is persisted by the caller; the instance is
    /// used only for the current session.
    pub fn set_player_instance(&mut self, player: usize, instance: Option<u32>) {
        if player >= PORTS {
            return;
        }
        let entry = instance.and_then(|id| {
            self.controllers
                .iter()
                .find(|controller| controller.instance_id == id)
                .cloned()
        });
        if let Some(entry) = &entry {
            let other = 1 - player;
            if self.players[other].instance == Some(entry.instance_id) {
                log::warn!(
                    "Controller {} is already assigned to player {}",
                    entry.name,
                    other + 1
                );
                return;
            }
        }

        self.players[player].controller = None;
        self.players[player].instance = None;
        match entry {
            Some(entry) => {
                self.players[player].guid = entry.guid.clone();
                self.open_player(player, entry.index);
            }
            None => {
                self.players[player].guid.clear();
            }
        }
    }

    /// Re-enumerate controllers and rebind both players.
    pub fn refresh_controllers(&mut self) {
        self.close_players();
        self.controllers.clear();

        let count = self.game_controllers.num_joysticks().unwrap_or(0);
        for index in 0..count {
            if !self.game_controllers.is_game_controller(index) {
                continue;
            }
            // Opening each controller briefly is the portable way to get its
            // instance ID and GUID through SDL's GameController API.
            if let Ok(controller) = self.game_controllers.open(index) {
                let instance_id = controller.instance_id();
                let mapping = controller.mapping();
                let guid = parse_guid(&mapping);
                let name = controller.name();
                log::debug!("controller {name} mapping: {mapping}");
                drop(controller);
                self.controllers.push(ControllerEntry {
                    index,
                    instance_id,
                    name,
                    guid,
                });
            }
        }

        self.reconcile_players();
    }

    fn close_players(&mut self) {
        for player in &mut self.players {
            player.controller = None;
            player.instance = None;
        }
    }

    fn open_player(&mut self, player: usize, index: u32) {
        match self.game_controllers.open(index) {
            Ok(controller) => {
                self.players[player].instance = Some(controller.instance_id());
                self.players[player].controller = Some(controller);
            }
            Err(e) => {
                log::warn!("Could not open controller for player {}: {e}", player + 1);
            }
        }
    }

    fn desired_entry(&self, player: usize, used: &[Option<u32>; PORTS]) -> Option<ControllerEntry> {
        let guid = &self.players[player].guid;
        // Player 2 is opt-in: an empty GUID means no second controller.
        if player > 0 && guid.is_empty() {
            return None;
        }
        self.controllers
            .iter()
            .find(|entry| {
                let free = !used
                    .iter()
                    .flatten()
                    .any(|instance| *instance == entry.instance_id);
                free && (guid.is_empty() || entry.guid == *guid)
            })
            .cloned()
    }

    fn reconcile_players(&mut self) {
        let mut desired: [Option<ControllerEntry>; PORTS] = [None, None];
        let mut used: [Option<u32>; PORTS] = [None; PORTS];

        for player in 0..PORTS {
            let entry = self.desired_entry(player, &used);
            used[player] = entry.as_ref().map(|entry| entry.instance_id);
            desired[player] = entry;
        }

        if self.players[0].guid.is_empty() {
            if let Some(entry) = &desired[0] {
                self.players[0].guid = entry.guid.clone();
            }
        }

        for (player, entry) in desired.iter().enumerate() {
            let wanted = entry.as_ref().map(|entry| entry.instance_id);
            if wanted == self.players[player].instance && self.players[player].controller.is_some()
            {
                continue;
            }
            self.players[player].controller = None;
            self.players[player].instance = None;
            if let Some(entry) = entry {
                self.open_player(player, entry.index);
            }
        }
    }

    fn player_for_instance(&self, instance: u32) -> Option<usize> {
        self.players
            .iter()
            .position(|player| player.instance == Some(instance))
    }

    /// Player 1 drives the UI. If only Player 2 is connected, fall back to it
    /// so a single assigned pad can still navigate.
    fn ui_player(&self) -> Option<usize> {
        if self.players[0].controller.is_some() {
            Some(0)
        } else if self.players[1].controller.is_some() {
            Some(1)
        } else {
            None
        }
    }

    /// Poll SDL, update the shared input snapshots, and return true when the
    /// menu button was pressed this frame.
    pub fn poll(&mut self, host: &Arc<HostState>, config: &Config) -> bool {
        let mut menu_pressed = false;
        let mut removed: Option<(u32, Option<usize>)> = None;

        let events: Vec<Event> = self.event_pump.poll_iter().collect();
        for event in events {
            match event {
                Event::ControllerDeviceAdded { which, .. } => {
                    self.refresh_controllers();
                    let name = self
                        .controllers
                        .iter()
                        .find(|controller| controller.instance_id == which)
                        .map(|controller| controller.name.clone());
                    host.notify(
                        match name {
                            Some(name) => format!("Controller connected: {name}"),
                            None => "Controller connected".to_string(),
                        },
                        MessageKind::Info,
                        Duration::from_secs(3),
                    );
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    removed = Some((which, self.player_for_instance(which)));
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    self.activity = true;
                    if let Some(player) = self.player_for_instance(which) {
                        self.last_button[player] = format!("{button:?}");
                        if Some(player) == self.ui_player() && button == Button::Guide {
                            log::info!("Player {} PS/Guide button pressed", player + 1);
                            menu_pressed = true;
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some((_which, player)) = removed {
            host.notify(
                match player {
                    Some(player) => format!("Player {} controller disconnected", player + 1),
                    None => "Controller disconnected".to_string(),
                },
                MessageKind::Warning,
                Duration::from_secs(4),
            );
            self.refresh_controllers();
        }

        self.update_input(host, config);

        if config.input.menu_button == "select_start" {
            let combo = self
                .ui_player()
                .and_then(|player| self.players[player].controller.as_ref())
                .map(|controller| {
                    controller.button(Button::Back) && controller.button(Button::Start)
                })
                .unwrap_or(false);
            if combo && !self.combo_was_down {
                menu_pressed = true;
            }
            self.combo_was_down = combo;
        }

        if menu_pressed {
            let allowed = self
                .last_menu_toggle
                .map(|t| t.elapsed() > Duration::from_millis(300))
                .unwrap_or(true);
            if allowed {
                self.last_menu_toggle = Some(Instant::now());
            } else {
                menu_pressed = false;
            }
        }

        self.update_rumble(host);
        menu_pressed
    }

    /// Edge-triggered navigation for the in-game overlay. D-pad and left stick
    /// move, Cross (SDL A) confirms, Circle (SDL B) cancels. Held directions
    /// repeat after a short delay. Input suppression also consumes navigation
    /// until the dismissing press is released.
    pub fn poll_menu_nav(&mut self, active: bool) -> Vec<NavAction> {
        if !active {
            self.nav_prev = [false; 6];
            self.nav_repeat_at = None;
            return Vec::new();
        }
        let Some(player) = self.ui_player() else {
            self.nav_prev = [false; 6];
            self.nav_repeat_at = None;
            return Vec::new();
        };
        let Some(controller) = self.players[player].controller.as_ref() else {
            self.nav_prev = [false; 6];
            self.nav_repeat_at = None;
            return Vec::new();
        };
        let state = [
            controller.button(Button::DPadUp) || controller.axis(Axis::LeftY) < -16000,
            controller.button(Button::DPadDown) || controller.axis(Axis::LeftY) > 16000,
            controller.button(Button::DPadLeft) || controller.axis(Axis::LeftX) < -16000,
            controller.button(Button::DPadRight) || controller.axis(Axis::LeftX) > 16000,
            controller.button(Button::A),
            controller.button(Button::B),
        ];
        if self.suppress_input {
            self.nav_prev = state;
            self.nav_repeat_at = None;
            return Vec::new();
        }
        let actions = [
            NavAction::Up,
            NavAction::Down,
            NavAction::Left,
            NavAction::Right,
            NavAction::Confirm,
            NavAction::Cancel,
        ];
        let now = Instant::now();
        let mut out = Vec::new();
        let repeat_due = self.nav_repeat_at.map(|t| now >= t).unwrap_or(false);
        for i in 0..6 {
            if state[i] && !self.nav_prev[i] {
                out.push(actions[i]);
                if i < 4 {
                    self.nav_repeat_at = Some(now + Duration::from_millis(320));
                }
            } else if state[i] && self.nav_prev[i] && i < 4 && repeat_due {
                out.push(actions[i]);
                self.nav_repeat_at = Some(now + Duration::from_millis(110));
            }
        }
        self.nav_prev = state;
        out
    }

    fn update_input(&mut self, host: &Arc<HostState>, config: &Config) {
        let snapshots: [(u32, [i32; 4]); PORTS] = std::array::from_fn(|player| {
            self.players[player]
                .controller
                .as_ref()
                .map(|controller| read_pad(controller, config.input.deadzone))
                .unwrap_or((0, [0; 4]))
        });

        if self.suppress_input {
            let any_button = snapshots.iter().any(|(buttons, _)| *buttons != 0);
            let any_axis = snapshots
                .iter()
                .any(|(_, axes)| axes.iter().any(|axis| axis.abs() > 2000));
            let expired = self
                .suppress_deadline
                .map(|deadline| Instant::now() >= deadline)
                .unwrap_or(true);
            if (any_button || any_axis) && !expired {
                host.input.clear();
                return;
            }
            self.clear_input_lockout();
        }

        for (port, (buttons, axes)) in snapshots.iter().enumerate() {
            host.input.set(port, *buttons, *axes);
        }
    }

    fn update_rumble(&mut self, host: &Arc<HostState>) {
        for player in 0..PORTS {
            let strong = host.rumble_strong[player].load(Ordering::Relaxed);
            let weak = host.rumble_weak[player].load(Ordering::Relaxed);
            let changed = (strong, weak) != self.last_rumble_values[player];
            let refresh = (strong != 0 || weak != 0)
                && self.last_rumble[player].elapsed() > Duration::from_millis(100);
            if !changed && !refresh {
                continue;
            }
            if let Some(controller) = self.players[player].controller.as_mut() {
                let duration = if strong == 0 && weak == 0 { 0 } else { 250 };
                if let Err(e) = controller.set_rumble(strong, weak, duration) {
                    log::debug!("Player {} rumble failed: {e}", player + 1);
                }
                self.last_rumble[player] = Instant::now();
                self.last_rumble_values[player] = (strong, weak);
            }
        }
    }
}

fn read_pad(controller: &GameController, deadzone: f32) -> (u32, [i32; 4]) {
    let mut buttons = 0u32;
    let mut set = |button: Button, retro_id: u32| {
        if controller.button(button) {
            buttons |= 1u32 << retro_id;
        }
    };
    set(Button::A, ffi::RETRO_DEVICE_ID_JOYPAD_B);
    set(Button::B, ffi::RETRO_DEVICE_ID_JOYPAD_A);
    set(Button::X, ffi::RETRO_DEVICE_ID_JOYPAD_Y);
    set(Button::Y, ffi::RETRO_DEVICE_ID_JOYPAD_X);
    set(Button::Back, ffi::RETRO_DEVICE_ID_JOYPAD_SELECT);
    set(Button::Start, ffi::RETRO_DEVICE_ID_JOYPAD_START);
    set(Button::DPadUp, ffi::RETRO_DEVICE_ID_JOYPAD_UP);
    set(Button::DPadDown, ffi::RETRO_DEVICE_ID_JOYPAD_DOWN);
    set(Button::DPadLeft, ffi::RETRO_DEVICE_ID_JOYPAD_LEFT);
    set(Button::DPadRight, ffi::RETRO_DEVICE_ID_JOYPAD_RIGHT);
    set(Button::LeftShoulder, ffi::RETRO_DEVICE_ID_JOYPAD_L);
    set(Button::RightShoulder, ffi::RETRO_DEVICE_ID_JOYPAD_R);
    set(Button::LeftStick, ffi::RETRO_DEVICE_ID_JOYPAD_L3);
    set(Button::RightStick, ffi::RETRO_DEVICE_ID_JOYPAD_R3);

    if controller.axis(Axis::TriggerLeft) > 16384 {
        buttons |= 1u32 << ffi::RETRO_DEVICE_ID_JOYPAD_L2;
    }
    if controller.axis(Axis::TriggerRight) > 16384 {
        buttons |= 1u32 << ffi::RETRO_DEVICE_ID_JOYPAD_R2;
    }

    let dz = deadzone.clamp(0.0, 0.9) * 32767.0;
    let dead = |value: i16| -> i32 {
        let v = value as f32;
        if v.abs() < dz {
            0
        } else {
            (v - v.signum() * dz) as i32
        }
    };

    let axes = [
        dead(controller.axis(Axis::LeftX)),
        dead(controller.axis(Axis::LeftY)),
        dead(controller.axis(Axis::RightX)),
        dead(controller.axis(Axis::RightY)),
    ];
    (buttons, axes)
}

fn parse_guid(mapping: &str) -> String {
    mapping
        .split(',')
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
