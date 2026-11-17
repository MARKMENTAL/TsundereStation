use crate::libretro::ffi;
use crate::libretro::host::{
    audio_sample, audio_sample_batch, environment_callback, input_poll, input_state, video_refresh,
    CategoryDef, ControllerPortInfo, CoreOptionDef, HostState,
};
use anyhow::{bail, Context, Result};
use libloading::Library;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
    pub extensions: String,
    pub options: Vec<CoreOptionDef>,
    pub categories: Vec<CategoryDef>,
    pub ports: Vec<ControllerPortInfo>,
}

struct CoreApi {
    retro_init: unsafe extern "C" fn(),
    retro_deinit: unsafe extern "C" fn(),
    retro_get_system_info: unsafe extern "C" fn(*mut ffi::retro_system_info),
    retro_get_system_av_info: unsafe extern "C" fn(*mut ffi::retro_system_av_info),
    retro_set_controller_port_device: unsafe extern "C" fn(c_uint, c_uint),
    retro_reset: unsafe extern "C" fn(),
    retro_run: unsafe extern "C" fn(),
    retro_serialize_size: unsafe extern "C" fn() -> usize,
    retro_serialize: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    retro_unserialize: unsafe extern "C" fn(*const c_void, usize) -> bool,
    retro_load_game: unsafe extern "C" fn(*const ffi::retro_game_info) -> bool,
    retro_unload_game: unsafe extern "C" fn(),
    retro_get_memory_data: unsafe extern "C" fn(c_uint) -> *mut c_void,
    retro_get_memory_size: unsafe extern "C" fn(c_uint) -> usize,
    retro_set_environment: unsafe extern "C" fn(ffi::retro_environment_t),
    retro_set_video_refresh: unsafe extern "C" fn(ffi::retro_video_refresh_t),
    retro_set_audio_sample: unsafe extern "C" fn(ffi::retro_audio_sample_t),
    retro_set_audio_sample_batch: unsafe extern "C" fn(ffi::retro_audio_sample_batch_t),
    retro_set_input_poll: unsafe extern "C" fn(ffi::retro_input_poll_t),
    retro_set_input_state: unsafe extern "C" fn(ffi::retro_input_state_t),
    retro_api_version: unsafe extern "C" fn() -> c_uint,
}

unsafe fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T> {
    let symbol = lib.get::<T>(name).with_context(|| {
        format!(
            "missing libretro symbol {}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        )
    })?;
    Ok(*symbol)
}

impl CoreApi {
    unsafe fn load(lib: &Library) -> Result<Self> {
        Ok(Self {
            retro_init: sym(lib, b"retro_init\0")?,
            retro_deinit: sym(lib, b"retro_deinit\0")?,
            retro_get_system_info: sym(lib, b"retro_get_system_info\0")?,
            retro_get_system_av_info: sym(lib, b"retro_get_system_av_info\0")?,
            retro_set_controller_port_device: sym(lib, b"retro_set_controller_port_device\0")?,
            retro_reset: sym(lib, b"retro_reset\0")?,
            retro_run: sym(lib, b"retro_run\0")?,
            retro_serialize_size: sym(lib, b"retro_serialize_size\0")?,
            retro_serialize: sym(lib, b"retro_serialize\0")?,
            retro_unserialize: sym(lib, b"retro_unserialize\0")?,
            retro_load_game: sym(lib, b"retro_load_game\0")?,
            retro_unload_game: sym(lib, b"retro_unload_game\0")?,
            retro_get_memory_data: sym(lib, b"retro_get_memory_data\0")?,
            retro_get_memory_size: sym(lib, b"retro_get_memory_size\0")?,
            retro_set_environment: sym(lib, b"retro_set_environment\0")?,
            retro_set_video_refresh: sym(lib, b"retro_set_video_refresh\0")?,
            retro_set_audio_sample: sym(lib, b"retro_set_audio_sample\0")?,
            retro_set_audio_sample_batch: sym(lib, b"retro_set_audio_sample_batch\0")?,
            retro_set_input_poll: sym(lib, b"retro_set_input_poll\0")?,
            retro_set_input_state: sym(lib, b"retro_set_input_state\0")?,
            retro_api_version: sym(lib, b"retro_api_version\0")?,
        })
    }
}

pub struct CoreHost {
    _lib: Library,
    api: CoreApi,
    host: Arc<HostState>,
    game: Option<PathBuf>,
    loaded: bool,
    port_device: Option<u32>,
    core_info: CoreInfo,
}

impl CoreHost {
    pub fn new(
        core_path: &Path,
        host: Arc<HostState>,
        saved_options: &BTreeMap<String, String>,
    ) -> Result<Self> {
        unsafe {
            let lib = Library::new(core_path)
                .with_context(|| format!("loading {}", core_path.display()))?;
            let api = CoreApi::load(&lib)?;
            let api_version = (api.retro_api_version)();
            if api_version != 1 {
                bail!("unsupported libretro API version {api_version}");
            }

            let mut sys_info = std::mem::zeroed::<ffi::retro_system_info>();
            (api.retro_get_system_info)(&mut sys_info);

            (api.retro_set_environment)(environment_callback);
            // The core has now registered its options; apply user overrides
            // before retro_init(), which reads them in LoadSettings().
            host.apply_saved_options(saved_options);

            (api.retro_set_video_refresh)(video_refresh);
            (api.retro_set_audio_sample)(audio_sample);
            (api.retro_set_audio_sample_batch)(audio_sample_batch);
            (api.retro_set_input_poll)(input_poll);
            (api.retro_set_input_state)(input_state);

            (api.retro_init)();

            let core_info = CoreInfo {
                name: cstr_owned(sys_info.library_name),
                version: cstr_owned(sys_info.library_version),
                extensions: cstr_owned(sys_info.valid_extensions),
                options: host.dump_options(),
                categories: host.dump_categories(),
                ports: host.controller_ports(),
            };

            Ok(Self {
                _lib: lib,
                api,
                host,
                game: None,
                loaded: false,
                port_device: None,
                core_info,
            })
        }
    }

    pub fn info(&self) -> &CoreInfo {
        &self.core_info
    }

    pub fn game(&self) -> Option<&Path> {
        self.game.as_deref()
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn load_game(&mut self, path: Option<&Path>, port_device: Option<u32>) -> Result<()> {
        if self.loaded {
            self.unload_game()?;
        }
        unsafe {
            if let Some(device) = port_device {
                (self.api.retro_set_controller_port_device)(0, device);
                self.port_device = Some(device);
            }

            let c_path = match path {
                Some(p) => Some(
                    CString::new(p.to_string_lossy().as_bytes())
                        .context("game path contains a NUL byte")?,
                ),
                None => None,
            };
            let game_info = ffi::retro_game_info {
                path: c_path
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(std::ptr::null()),
                data: std::ptr::null(),
                size: 0,
                meta: std::ptr::null(),
            };
            let game_ptr = if path.is_some() {
                &game_info as *const ffi::retro_game_info
            } else {
                std::ptr::null()
            };
            if !(self.api.retro_load_game)(game_ptr) {
                bail!(
                    "retro_load_game failed for {}",
                    path.map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<BIOS>".into())
                );
            }

            let mut av = std::mem::zeroed::<ffi::retro_system_av_info>();
            (self.api.retro_get_system_av_info)(&mut av);
            if av.timing.fps > 1.0 && av.timing.fps < 1000.0 {
                self.host.fps_bits.store(
                    av.timing.fps.to_bits(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            self.host
                .base_width
                .store(av.geometry.base_width, std::sync::atomic::Ordering::Relaxed);
            self.host.base_height.store(
                av.geometry.base_height,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        self.game = path.map(|p| p.to_path_buf());
        self.loaded = true;
        self.load_memory_card();
        Ok(())
    }

    pub fn unload_game(&mut self) -> Result<()> {
        if !self.loaded {
            return Ok(());
        }
        self.save_memory_card();
        unsafe {
            (self.api.retro_unload_game)();
        }
        self.loaded = false;
        Ok(())
    }

    pub fn reset(&mut self) {
        if self.loaded {
            unsafe { (self.api.retro_reset)() };
        }
    }

    pub fn set_port_device(&mut self, port: u32, device: u32) {
        if self.loaded {
            unsafe { (self.api.retro_set_controller_port_device)(port, device) };
            if port == 0 {
                self.port_device = Some(device);
            }
        }
    }

    pub fn run_frame(&mut self) {
        if self.loaded {
            unsafe { (self.api.retro_run)() };
        }
    }

    pub fn serialize_size(&self) -> usize {
        unsafe { (self.api.retro_serialize_size)() }
    }

    pub fn save_state(&self) -> Result<Vec<u8>> {
        if !self.loaded {
            bail!("no game loaded");
        }
        let size = self.serialize_size();
        if size == 0 {
            bail!("core reported a zero-sized save state");
        }
        let mut data = vec![0u8; size];
        let ok =
            unsafe { (self.api.retro_serialize)(data.as_mut_ptr() as *mut c_void, data.len()) };
        if !ok {
            bail!("retro_serialize failed");
        }
        Ok(data)
    }

    pub fn save_state_to(&self, path: &Path) -> Result<()> {
        let data = self.save_state()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn load_state_from(&mut self, path: &Path) -> Result<()> {
        if !self.loaded {
            bail!("no game loaded");
        }
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if data.is_empty() {
            bail!("save state is empty");
        }
        let ok =
            unsafe { (self.api.retro_unserialize)(data.as_ptr() as *const c_void, data.len()) };
        if !ok {
            bail!("retro_unserialize failed");
        }
        Ok(())
    }

    pub fn memory_card_slice(&mut self) -> Option<&mut [u8]> {
        if !self.loaded {
            return None;
        }
        unsafe {
            let ptr = (self.api.retro_get_memory_data)(ffi::RETRO_MEMORY_SAVE_RAM);
            let size = (self.api.retro_get_memory_size)(ffi::RETRO_MEMORY_SAVE_RAM);
            if ptr.is_null() || size != 128 * 1024 {
                return None;
            }
            Some(std::slice::from_raw_parts_mut(ptr as *mut u8, size))
        }
    }

    pub fn save_memory_card(&mut self) {
        let game = self.game.clone().unwrap_or_else(|| PathBuf::from("bios"));
        let path = memory_card_path(&self.host.save_dir, &game);
        if let Some(card) = self.memory_card_slice() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&path, card) {
                log::warn!("Could not save memory card {}: {e}", path.display());
            }
        }
    }

    pub fn load_memory_card(&mut self) {
        let game = self.game.clone().unwrap_or_else(|| PathBuf::from("bios"));
        let path = memory_card_path(&self.host.save_dir, &game);
        let Ok(data) = std::fs::read(&path) else {
            return;
        };
        if let Some(card) = self.memory_card_slice() {
            if data.len() == card.len() {
                card.copy_from_slice(&data);
                log::info!("Loaded memory card {}", path.display());
            } else {
                log::warn!(
                    "Ignoring memory card {}: expected {} bytes, got {}",
                    path.display(),
                    card.len(),
                    data.len()
                );
            }
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.unload_game();
        unsafe { (self.api.retro_deinit)() };
    }
}

fn cstr_owned(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

pub fn sanitize_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "game".to_string());
    let mut out = String::with_capacity(stem.len());
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        "game".to_string()
    } else {
        trimmed
    }
}

pub fn state_path(state_dir: &Path, game: &Path, slot: u32) -> PathBuf {
    let stem = sanitize_stem(game);
    if slot == 0 {
        state_dir.join(format!("{stem}.state"))
    } else {
        state_dir.join(format!("{stem}.state{slot}"))
    }
}

pub fn memory_card_path(save_dir: &Path, game: &Path) -> PathBuf {
    save_dir.join(format!("{}.srm", sanitize_stem(game)))
}
