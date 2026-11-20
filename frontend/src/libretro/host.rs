use crate::libretro::audio::AudioRing;
use crate::libretro::ffi;
use crate::libretro::input::InputShared;
use crate::libretro::video::VideoMailbox;
use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

pub static HOST: OnceLock<Arc<HostState>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct CoreOptionDef {
    pub key: String,
    pub label: String,
    pub info: String,
    pub category_key: String,
    pub values: Vec<(String, String)>,
    pub default: String,
}

#[derive(Clone, Debug)]
pub struct CategoryDef {
    pub key: String,
    pub label: String,
    pub info: String,
}

#[derive(Clone, Debug, Default)]
pub struct ControllerPortInfo {
    pub types: Vec<(String, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Warning,
    Error,
    Debug,
}

#[derive(Clone, Copy)]
pub struct DiskControl {
    pub set_eject_state: Option<ffi::retro_set_eject_state_t>,
    pub get_image_index: Option<ffi::retro_get_image_index_t>,
    pub set_image_index: Option<ffi::retro_set_image_index_t>,
    pub get_num_images: Option<ffi::retro_get_num_images_t>,
    pub get_image_path: Option<ffi::retro_get_image_path_t>,
    pub get_image_label: Option<ffi::retro_get_image_label_t>,
}

impl DiskControl {
    fn from_ext(callback: &ffi::retro_disk_control_ext_callback) -> Self {
        Self {
            set_eject_state: callback.set_eject_state,
            get_image_index: callback.get_image_index,
            set_image_index: callback.set_image_index,
            get_num_images: callback.get_num_images,
            get_image_path: callback.get_image_path,
            get_image_label: callback.get_image_label,
        }
    }

    fn from_legacy(callback: &ffi::retro_disk_control_callback) -> Self {
        Self {
            set_eject_state: callback.set_eject_state,
            get_image_index: callback.get_image_index,
            set_image_index: callback.set_image_index,
            get_num_images: callback.get_num_images,
            get_image_path: None,
            get_image_label: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostMessage {
    pub text: String,
    pub kind: MessageKind,
    pub created: Instant,
    pub ttl: Duration,
}

#[derive(Default)]
pub struct OptionStore {
    pub categories: Vec<CategoryDef>,
    pub defs: Vec<CoreOptionDef>,
    pub by_key: HashMap<String, usize>,
    pub overrides: HashMap<String, String>,
    pub visible: HashMap<String, bool>,
    pub generation: u64,
    pub last_seen: u64,
}

impl OptionStore {
    pub fn def(&self, key: &str) -> Option<&CoreOptionDef> {
        self.by_key.get(key).map(|&i| &self.defs[i])
    }

    pub fn current(&self, key: &str) -> Option<String> {
        if key == "swanstation_GPU_Renderer" {
            return Some("Software".to_string());
        }
        if let Some(v) = self.overrides.get(key) {
            return Some(v.clone());
        }
        self.def(key).map(|d| d.default.clone())
    }

    pub fn set_override(&mut self, key: &str, value: String) {
        let default = self.def(key).map(|d| d.default.clone());
        if default.as_deref() == Some(value.as_str()) {
            self.overrides.remove(key);
        } else {
            self.overrides.insert(key.to_string(), value);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn reset(&mut self, key: &str) {
        self.overrides.remove(key);
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn reset_all(&mut self) {
        self.overrides.clear();
        self.generation = self.generation.wrapping_add(1);
    }
}

pub struct HostState {
    pub save_dir: PathBuf,
    system_dir_c: CString,
    save_dir_c: CString,
    assets_dir_c: CString,

    pub options: RwLock<OptionStore>,
    interned: Mutex<Vec<CString>>,

    pub video: Arc<VideoMailbox>,
    pub audio: Arc<AudioRing>,
    pub input: Arc<InputShared>,

    messages: Mutex<VecDeque<HostMessage>>,
    controller_ports: Mutex<Vec<ControllerPortInfo>>,

    pub shutdown: AtomicBool,
    pub fast_forward: AtomicBool,
    /// Core-reported nominal refresh rate, used for frame pacing.
    pub fps_bits: AtomicU64,
    /// Monotonic count of frames actually run by the emulation worker.
    pub emulated_frames: AtomicU64,
    pub base_width: AtomicU32,
    pub base_height: AtomicU32,

    pub rumble_strong: [AtomicU16; crate::libretro::input::PORTS],
    pub rumble_weak: [AtomicU16; crate::libretro::input::PORTS],
    pub rumble_generation: [AtomicU64; crate::libretro::input::PORTS],

    disk_control: Mutex<Option<DiskControl>>,
}

impl HostState {
    pub fn new(system_dir: &Path, save_dir: &Path) -> Self {
        let system_dir_c = path_cstring(system_dir);
        let save_dir_c = path_cstring(save_dir);
        let assets_dir_c = system_dir_c.clone();
        Self {
            save_dir: save_dir.to_path_buf(),
            system_dir_c,
            save_dir_c,
            assets_dir_c,
            options: RwLock::new(OptionStore::default()),
            interned: Mutex::new(Vec::new()),
            video: Arc::new(VideoMailbox::new()),
            audio: Arc::new(AudioRing::new()),
            input: Arc::new(InputShared::new()),
            messages: Mutex::new(VecDeque::new()),
            controller_ports: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            fast_forward: AtomicBool::new(false),
            fps_bits: AtomicU64::new(60.0f64.to_bits()),
            emulated_frames: AtomicU64::new(0),
            base_width: AtomicU32::new(320),
            base_height: AtomicU32::new(240),
            rumble_strong: std::array::from_fn(|_| AtomicU16::new(0)),
            rumble_weak: std::array::from_fn(|_| AtomicU16::new(0)),
            rumble_generation: std::array::from_fn(|_| AtomicU64::new(0)),
            disk_control: Mutex::new(None),
        }
    }

    pub fn fps(&self) -> f64 {
        f64::from_bits(self.fps_bits.load(Ordering::Relaxed)).max(1.0)
    }

    pub fn emulated_frames(&self) -> u64 {
        self.emulated_frames.load(Ordering::Relaxed)
    }

    pub fn notify(&self, text: impl Into<String>, kind: MessageKind, ttl: Duration) {
        self.push_message(text.into(), kind, ttl);
    }

    pub fn take_messages(&self) -> Vec<HostMessage> {
        self.messages.lock().unwrap().drain(..).collect()
    }

    pub fn controller_ports(&self) -> Vec<ControllerPortInfo> {
        self.controller_ports.lock().unwrap().clone()
    }

    pub fn set_disk_control_ext(&self, callback: &ffi::retro_disk_control_ext_callback) {
        *self.disk_control.lock().unwrap() = Some(DiskControl::from_ext(callback));
    }

    pub fn set_disk_control_legacy(&self, callback: &ffi::retro_disk_control_callback) {
        *self.disk_control.lock().unwrap() = Some(DiskControl::from_legacy(callback));
    }

    pub fn clear_disk_control(&self) {
        *self.disk_control.lock().unwrap() = None;
    }

    pub fn disk_control(&self) -> Option<DiskControl> {
        *self.disk_control.lock().unwrap()
    }

    pub fn intern(&self, value: &str) -> *const c_char {
        let cleaned = value.replace('\0', "");
        let mut values = self.interned.lock().unwrap();
        if let Some(existing) = values.iter().find(|c| c.as_bytes() == cleaned.as_bytes()) {
            return existing.as_ptr();
        }
        let c = CString::new(cleaned).unwrap_or_default();
        let ptr = c.as_ptr();
        values.push(c);
        ptr
    }

    pub fn option_value(&self, key: &str) -> Option<String> {
        self.options.read().unwrap().current(key)
    }

    pub fn set_option(&self, key: &str, value: String) {
        self.options.write().unwrap().set_override(key, value);
    }

    pub fn reset_option(&self, key: &str) {
        self.options.write().unwrap().reset(key);
    }

    fn push_message(&self, text: String, kind: MessageKind, ttl: Duration) {
        let mut messages = self.messages.lock().unwrap();
        if messages.len() >= 20 {
            messages.pop_front();
        }
        messages.push_back(HostMessage {
            text,
            kind,
            created: Instant::now(),
            ttl,
        });
    }

    unsafe fn handle_env(&self, cmd: c_uint, data: *mut c_void) -> bool {
        match cmd {
            ffi::RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut *const c_char) = self.system_dir_c.as_ptr();
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut *const c_char) = self.save_dir_c.as_ptr();
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_CORE_ASSETS_DIRECTORY => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut *const c_char) = self.assets_dir_c.as_ptr();
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_LANGUAGE => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = ffi::RETRO_LANGUAGE_ENGLISH;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 2;
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL => {
                if data.is_null() {
                    return false;
                }
                let intl = &*(data as *const ffi::retro_core_options_v2_intl);
                if intl.us.is_null() {
                    return false;
                }
                self.parse_core_options(&*intl.us);
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2 => {
                if data.is_null() {
                    return false;
                }
                self.parse_core_options(&*(data as *const ffi::retro_core_options_v2));
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY => {
                if data.is_null() {
                    return true;
                }
                let d = &*(data as *const ffi::retro_core_option_display);
                if !d.key.is_null() {
                    let key = cstr(d.key);
                    self.options.write().unwrap().visible.insert(key, d.visible);
                }
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_VARIABLE => {
                if data.is_null() {
                    return true;
                }
                let var = &mut *(data as *mut ffi::retro_variable);
                if var.key.is_null() {
                    var.value = std::ptr::null();
                    return true;
                }
                let key = cstr(var.key);
                match self.option_value(&key) {
                    Some(value) => {
                        var.value = self.intern(&value);
                    }
                    None => {
                        var.value = std::ptr::null();
                    }
                }
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
                if data.is_null() {
                    return false;
                }
                let mut options = self.options.write().unwrap();
                let changed = options.generation != options.last_seen;
                options.last_seen = options.generation;
                *(data as *mut bool) = changed;
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                if data.is_null() {
                    return false;
                }
                self.video.set_format(*(data as *const c_int))
            }
            ffi::RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME => {
                if !data.is_null() {
                    *(data as *mut bool) = true;
                }
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS => true,
            ffi::RETRO_ENVIRONMENT_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK => true,
            ffi::RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL => true,
            ffi::RETRO_ENVIRONMENT_GET_INPUT_BITMASKS => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 1;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE => {
                if data.is_null() {
                    return false;
                }
                let flags = if self.fast_forward.load(Ordering::Relaxed) {
                    ffi::RETRO_AV_ENABLE_VIDEO
                } else {
                    ffi::RETRO_AV_ENABLE_VIDEO | ffi::RETRO_AV_ENABLE_AUDIO
                };
                *(data as *mut c_int) = flags;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_SAVESTATE_CONTEXT => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_int) = 0;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_FASTFORWARDING => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut bool) = self.fast_forward.load(Ordering::Relaxed);
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE => {
                if data.is_null() {
                    return false;
                }
                let iface = &mut *(data as *mut ffi::retro_rumble_interface);
                iface.set_rumble_state = Some(rumble_callback);
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_PREFERRED_HW_RENDER => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 0;
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO => {
                if data.is_null() {
                    return false;
                }
                let av = &*(data as *const ffi::retro_system_av_info);
                self.apply_av_info(av);
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_GEOMETRY => {
                if data.is_null() {
                    return false;
                }
                let geo = &*(data as *const ffi::retro_game_geometry);
                self.base_width.store(geo.base_width, Ordering::Relaxed);
                self.base_height.store(geo.base_height, Ordering::Relaxed);
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_CONTROLLER_INFO => {
                if data.is_null() {
                    return false;
                }
                self.parse_controller_info(data as *const ffi::retro_controller_info);
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_MESSAGE => {
                if data.is_null() {
                    return true;
                }
                let msg = &*(data as *const ffi::retro_message);
                if !msg.msg.is_null() {
                    let text = cstr(msg.msg);
                    if !text.is_empty() {
                        let frames = if msg.frames == 0 { 120 } else { msg.frames };
                        let ttl = Duration::from_secs_f64(frames as f64 / self.fps());
                        self.push_message(text, MessageKind::Info, ttl);
                    }
                }
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_MESSAGE_EXT => {
                if data.is_null() {
                    return true;
                }
                let msg = &*(data as *const ffi::retro_message_ext);
                if msg.msg.is_null() {
                    return true;
                }
                let text = cstr(msg.msg);
                let kind = match msg.level {
                    0 => MessageKind::Error,
                    1 => MessageKind::Warning,
                    3 => MessageKind::Debug,
                    _ => MessageKind::Info,
                };
                if msg.target == ffi::RETRO_MESSAGE_TARGET_LOG {
                    match kind {
                        MessageKind::Error => log::error!("{text}"),
                        MessageKind::Warning => log::warn!("{text}"),
                        _ => log::info!("{text}"),
                    }
                } else {
                    let ttl = Duration::from_millis(if msg.duration == 0 {
                        2000
                    } else {
                        msg.duration as u64
                    });
                    if !text.is_empty() {
                        self.push_message(text, kind, ttl);
                    }
                }
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_MESSAGE_INTERFACE_VERSION => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 1;
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_HW_RENDER => false,
            ffi::RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER => false,
            ffi::RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE => {
                if data.is_null() {
                    self.clear_disk_control();
                    return true;
                }
                let callback = &*(data as *const ffi::retro_disk_control_callback);
                self.set_disk_control_legacy(callback);
                true
            }
            ffi::RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE => {
                if data.is_null() {
                    self.clear_disk_control();
                    return true;
                }
                let callback = &*(data as *const ffi::retro_disk_control_ext_callback);
                self.set_disk_control_ext(callback);
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 1;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_VFS_INTERFACE => false,
            ffi::RETRO_ENVIRONMENT_GET_LOG_INTERFACE => false,
            ffi::RETRO_ENVIRONMENT_GET_INPUT_MAX_USERS => {
                if data.is_null() {
                    return false;
                }
                *(data as *mut c_uint) = 1;
                true
            }
            ffi::RETRO_ENVIRONMENT_GET_THROTTLE_STATE => false,
            ffi::RETRO_ENVIRONMENT_SHUTDOWN => {
                self.shutdown.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    fn apply_av_info(&self, av: &ffi::retro_system_av_info) {
        if av.timing.fps > 1.0 && av.timing.fps < 1000.0 {
            self.fps_bits
                .store(av.timing.fps.to_bits(), Ordering::Relaxed);
        }
        self.base_width
            .store(av.geometry.base_width, Ordering::Relaxed);
        self.base_height
            .store(av.geometry.base_height, Ordering::Relaxed);
    }

    unsafe fn parse_controller_info(&self, ports: *const ffi::retro_controller_info) {
        let mut out = Vec::new();
        let mut port = ports;
        loop {
            let info = &*port;
            if info.types.is_null() || info.num_types == 0 {
                break;
            }
            let mut types = Vec::new();
            for i in 0..info.num_types as usize {
                let t = &*info.types.add(i);
                let name = if t.desc.is_null() {
                    String::new()
                } else {
                    cstr(t.desc)
                };
                // num_types includes the zeroed terminator in this core.
                if name.is_empty() || t.id == 0 {
                    continue;
                }
                types.push((name, t.id));
            }
            out.push(ControllerPortInfo { types });
            port = port.add(1);
            if out.len() >= 8 {
                break;
            }
        }
        *self.controller_ports.lock().unwrap() = out;
    }

    unsafe fn parse_core_options(&self, v2: &ffi::retro_core_options_v2) {
        let mut store = OptionStore::default();

        if !v2.categories.is_null() {
            let mut cat = v2.categories;
            loop {
                let c = &*cat;
                if c.key.is_null() {
                    break;
                }
                store.categories.push(CategoryDef {
                    key: cstr(c.key),
                    label: if c.desc.is_null() {
                        String::new()
                    } else {
                        cstr(c.desc)
                    },
                    info: if c.info.is_null() {
                        String::new()
                    } else {
                        cstr(c.info)
                    },
                });
                cat = cat.add(1);
            }
        }

        if !v2.definitions.is_null() {
            let mut def = v2.definitions;
            loop {
                let d = &*def;
                if d.key.is_null() {
                    break;
                }
                let key = cstr(d.key);
                let label = {
                    let categorized = if d.desc_categorized.is_null() {
                        String::new()
                    } else {
                        cstr(d.desc_categorized)
                    };
                    if !categorized.is_empty() {
                        categorized
                    } else if !d.desc.is_null() {
                        let desc = cstr(d.desc);
                        if desc.is_empty() {
                            key.clone()
                        } else {
                            desc
                        }
                    } else {
                        key.clone()
                    }
                };
                let info = {
                    let categorized = if d.info_categorized.is_null() {
                        String::new()
                    } else {
                        cstr(d.info_categorized)
                    };
                    if !categorized.is_empty() {
                        categorized
                    } else if !d.info.is_null() {
                        cstr(d.info)
                    } else {
                        String::new()
                    }
                };
                let category_key = if d.category_key.is_null() {
                    String::new()
                } else {
                    cstr(d.category_key)
                };
                let default = if d.default_value.is_null() {
                    String::new()
                } else {
                    cstr(d.default_value)
                };
                let mut values = Vec::new();
                for i in 0..ffi::RETRO_NUM_CORE_OPTION_VALUES_MAX {
                    let v = &d.values[i];
                    if v.value.is_null() {
                        break;
                    }
                    let value = cstr(v.value);
                    let vlabel = if v.label.is_null() {
                        value.clone()
                    } else {
                        cstr(v.label)
                    };
                    values.push((value, vlabel));
                }
                let index = store.defs.len();
                store.by_key.insert(key.clone(), index);
                store.defs.push(CoreOptionDef {
                    key,
                    label,
                    info,
                    category_key,
                    values,
                    default,
                });
                def = def.add(1);
            }
        }

        *self.options.write().unwrap() = store;
    }

    pub fn apply_saved_options(&self, saved: &std::collections::BTreeMap<String, String>) {
        let mut store = self.options.write().unwrap();
        for (key, value) in saved {
            if let Some(def) = store.def(key) {
                // BIOS paths accept any filename within the system directory
                // even though the core option only advertises the common names.
                let custom_allowed = key.starts_with("swanstation_BIOS_Path");
                if custom_allowed || def.values.iter().any(|(v, _)| v == value) {
                    store.overrides.insert(key.clone(), value.clone());
                }
            }
        }
        store.generation = store.generation.wrapping_add(1);
    }

    pub fn dump_options(&self) -> Vec<CoreOptionDef> {
        self.options.read().unwrap().defs.clone()
    }

    pub fn dump_categories(&self) -> Vec<CategoryDef> {
        self.options.read().unwrap().categories.clone()
    }
}

fn path_cstring(path: &Path) -> CString {
    CString::new(path.to_string_lossy().as_bytes())
        .unwrap_or_else(|_| CString::new("/tmp").unwrap())
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

pub extern "C" fn environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    match HOST.get() {
        Some(host) => unsafe { host.handle_env(cmd, data) },
        None => false,
    }
}

pub extern "C" fn video_refresh(data: *const c_void, width: c_uint, height: c_uint, pitch: usize) {
    if let Some(host) = HOST.get() {
        unsafe { host.video.push(data, width, height, pitch) };
    }
}

pub extern "C" fn audio_sample(left: i16, right: i16) {
    if let Some(host) = HOST.get() {
        host.audio.push_stereo(left, right);
    }
}

pub extern "C" fn audio_sample_batch(data: *const i16, frames: usize) -> usize {
    if frames == 0 || data.is_null() {
        return frames;
    }
    if let Some(host) = HOST.get() {
        let samples = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        host.audio.push_interleaved_i16(samples);
    }
    frames
}

pub extern "C" fn input_poll() {}

pub extern "C" fn input_state(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16 {
    let port = port as usize;
    if port >= crate::libretro::input::PORTS {
        return 0;
    }
    let Some(host) = HOST.get() else {
        return 0;
    };
    if device == ffi::RETRO_DEVICE_JOYPAD {
        if id == ffi::RETRO_DEVICE_ID_JOYPAD_MASK {
            return (host.input.buttons(port) & 0xffff) as i16;
        }
        if id < 16 {
            return ((host.input.buttons(port) >> id) & 1) as i16;
        }
    } else if device == ffi::RETRO_DEVICE_ANALOG {
        let idx = index as usize;
        let axis = id as usize;
        return host.input.axis(port, idx, axis);
    }
    0
}

pub extern "C" fn rumble_callback(port: c_uint, effect: c_int, strength: u16) -> bool {
    let port = port as usize;
    if port >= crate::libretro::input::PORTS {
        return false;
    }
    let Some(host) = HOST.get() else {
        return false;
    };
    match effect {
        0 => host.rumble_strong[port].store(strength, Ordering::Relaxed),
        1 => host.rumble_weak[port].store(strength, Ordering::Relaxed),
        _ => return false,
    }
    host.rumble_generation[port].fetch_add(1, Ordering::Relaxed);
    true
}
