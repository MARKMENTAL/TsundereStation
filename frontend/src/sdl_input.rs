use crate::config::Config;
use crate::libretro::audio::AudioRing;
use crate::libretro::ffi;
use crate::libretro::host::{HostState, MessageKind};
use anyhow::{anyhow, Result};
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::GameControllerSubsystem;
use std::sync::atomic::Ordering;
use std::sync::Arc;
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

pub struct RingAudio {
    ring: Arc<AudioRing>,
}

impl AudioCallback for RingAudio {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        self.ring.fill(out);
    }
}

pub struct SdlState {
    event_pump: sdl2::EventPump,
    game_controllers: GameControllerSubsystem,
    controllers: Vec<ControllerEntry>,
    selected: Option<GameController>,
    selected_guid: String,
    selected_instance: Option<u32>,
    combo_was_down: bool,
    last_menu_toggle: Option<Instant>,
    last_rumble: Instant,
    last_rumble_values: (u16, u16),
    last_button: String,
    nav_prev: [bool; 6],
    nav_repeat_at: Option<Instant>,
    // Keep the subsystem/device handles alive and drop them before SDL_Quit.
    _audio_device: Option<AudioDevice<RingAudio>>,
    _audio_subsystem: Option<sdl2::AudioSubsystem>,
    _sdl: sdl2::Sdl,
}

impl SdlState {
    pub fn new(ring: Arc<AudioRing>, preferred_guid: &str) -> Result<Self> {
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
                match subsystem.open_playback(None, &desired, move |_spec| RingAudio {
                    ring: ring_for_device,
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

        let mut state = Self {
            _sdl: sdl,
            _audio_subsystem: audio_subsystem,
            _audio_device: audio_device,
            event_pump,
            game_controllers,
            controllers: Vec::new(),
            selected: None,
            selected_guid: preferred_guid.to_string(),
            selected_instance: None,
            combo_was_down: false,
            last_menu_toggle: None,
            last_rumble: Instant::now(),
            last_rumble_values: (0, 0),
            last_button: String::new(),
            nav_prev: [false; 6],
            nav_repeat_at: None,
        };
        state.refresh_controllers(None);
        Ok(state)
    }

    pub fn controllers(&self) -> &[ControllerEntry] {
        &self.controllers
    }

    pub fn selected_guid(&self) -> &str {
        &self.selected_guid
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.controllers
            .iter()
            .find(|c| c.guid == self.selected_guid)
            .map(|c| c.name.as_str())
    }

    pub fn last_button(&self) -> &str {
        &self.last_button
    }

    pub fn select_guid(&mut self, guid: &str) {
        if self.selected_guid == guid && self.selected.is_some() {
            return;
        }
        self.selected_guid = guid.to_string();
        self.close_selected();
        self.open_selected();
    }

    /// Re-enumerate controllers. If `select_instance` is provided, that
    /// controller is preferred (used after a hotplug event).
    pub fn refresh_controllers(&mut self, select_instance: Option<u32>) {
        self.close_selected();
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

        if let Some(instance) = select_instance {
            if let Some(entry) = self.controllers.iter().find(|c| c.instance_id == instance) {
                self.selected_guid = entry.guid.clone();
            }
        }

        if self.selected_guid.is_empty() {
            if let Some(first) = self.controllers.first() {
                self.selected_guid = first.guid.clone();
            }
        }

        self.open_selected();
    }

    fn close_selected(&mut self) {
        self.selected = None;
        self.selected_instance = None;
    }

    fn open_selected(&mut self) {
        self.selected = None;
        self.selected_instance = None;
        if self.selected_guid.is_empty() {
            return;
        }
        let Some(entry) = self
            .controllers
            .iter()
            .find(|c| c.guid == self.selected_guid)
            .cloned()
        else {
            return;
        };
        match self.game_controllers.open(entry.index) {
            Ok(controller) => {
                self.selected_instance = Some(controller.instance_id());
                self.selected = Some(controller);
            }
            Err(e) => {
                log::warn!("Could not open controller {}: {e}", entry.name);
            }
        }
    }

    /// Poll SDL, update the shared input snapshot, and return true when the
    /// menu button was pressed this frame.
    pub fn poll(&mut self, host: &Arc<HostState>, config: &Config) -> bool {
        let mut menu_pressed = false;
        let mut removed_instance = None;

        let events: Vec<Event> = self.event_pump.poll_iter().collect();
        for event in events {
            match event {
                Event::ControllerDeviceAdded { which, .. } => {
                    let _ = which;
                    self.refresh_controllers(None);
                    if let Some(name) = self.selected_name().map(|s| s.to_string()) {
                        host.notify(
                            format!("Controller connected: {name}"),
                            MessageKind::Info,
                            Duration::from_secs(3),
                        );
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    removed_instance = Some(which);
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    if Some(which) == self.selected_instance {
                        self.last_button = format!("{button:?}");
                        if button == Button::Guide {
                            log::info!("PS/Guide button pressed");
                            menu_pressed = true;
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(instance) = removed_instance {
            let was_selected = self.selected_instance == Some(instance);
            if was_selected {
                host.notify(
                    "Controller disconnected",
                    MessageKind::Warning,
                    Duration::from_secs(4),
                );
            }
            self.refresh_controllers(None);
        }

        self.update_input(host, config);

        if config.input.menu_button == "select_start" {
            let combo = self
                .selected
                .as_ref()
                .map(|c| c.button(Button::Back) && c.button(Button::Start))
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
    /// repeat after a short delay.
    pub fn poll_menu_nav(&mut self, active: bool) -> Vec<NavAction> {
        if !active {
            self.nav_prev = [false; 6];
            self.nav_repeat_at = None;
            return Vec::new();
        }
        let Some(controller) = self.selected.as_ref() else {
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
        let Some(controller) = self.selected.as_ref() else {
            host.input.set(0, [0; 4]);
            return;
        };

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

        let dz = config.input.deadzone.clamp(0.0, 0.9) * 32767.0;
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
        host.input.set(buttons, axes);
    }

    fn update_rumble(&mut self, host: &Arc<HostState>) {
        let strong = host.rumble_strong.load(Ordering::Relaxed);
        let weak = host.rumble_weak.load(Ordering::Relaxed);
        let changed = (strong, weak) != self.last_rumble_values;
        let refresh =
            (strong != 0 || weak != 0) && self.last_rumble.elapsed() > Duration::from_millis(100);
        if !changed && !refresh {
            return;
        }
        if let Some(controller) = self.selected.as_mut() {
            let duration = if strong == 0 && weak == 0 { 0 } else { 250 };
            if let Err(e) = controller.set_rumble(strong, weak, duration) {
                log::debug!("Rumble failed: {e}");
            }
            self.last_rumble = Instant::now();
            self.last_rumble_values = (strong, weak);
        }
    }
}

fn parse_guid(mapping: &str) -> String {
    mapping
        .split(',')
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
