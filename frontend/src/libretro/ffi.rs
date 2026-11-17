//! Minimal libretro ABI definitions used by this frontend.
//!
//! These are deliberately hand-written against the copy of `libretro.h` in
//! `dep/libretro-common/include/libretro.h`, so the frontend does not need
//! bindgen or a matching external crate.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

pub type retro_environment_t = extern "C" fn(cmd: c_uint, data: *mut c_void) -> bool;
pub type retro_video_refresh_t =
    extern "C" fn(data: *const c_void, width: c_uint, height: c_uint, pitch: usize);
pub type retro_audio_sample_t = extern "C" fn(left: i16, right: i16);
pub type retro_audio_sample_batch_t = extern "C" fn(data: *const i16, frames: usize) -> usize;
pub type retro_input_poll_t = extern "C" fn();
pub type retro_input_state_t =
    extern "C" fn(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16;
pub type retro_set_rumble_state_t =
    extern "C" fn(port: c_uint, effect: c_int, strength: u16) -> bool;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_system_info {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_game_geometry {
    pub base_width: c_uint,
    pub base_height: c_uint,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_system_timing {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_system_av_info {
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_game_info {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_message {
    pub msg: *const c_char,
    pub frames: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_message_ext {
    pub msg: *const c_char,
    pub duration: c_uint,
    pub priority: c_uint,
    pub level: c_int,
    pub target: c_int,
    pub type_: c_int,
    pub progress: i8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_rumble_interface {
    pub set_rumble_state: Option<retro_set_rumble_state_t>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_option_value {
    pub value: *const c_char,
    pub label: *const c_char,
}

pub const RETRO_NUM_CORE_OPTION_VALUES_MAX: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_option_v2_definition {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub desc_categorized: *const c_char,
    pub info: *const c_char,
    pub info_categorized: *const c_char,
    pub category_key: *const c_char,
    pub values: [retro_core_option_value; RETRO_NUM_CORE_OPTION_VALUES_MAX],
    pub default_value: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_option_v2_category {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub info: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_options_v2 {
    pub categories: *const retro_core_option_v2_category,
    pub definitions: *const retro_core_option_v2_definition,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_options_v2_intl {
    pub us: *const retro_core_options_v2,
    pub local: *const retro_core_options_v2,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_option_display {
    pub key: *const c_char,
    pub visible: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_controller_description {
    pub desc: *const c_char,
    pub id: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_controller_info {
    pub types: *const retro_controller_description,
    pub num_types: c_uint,
}

// Environment command numbers.
pub const RETRO_ENVIRONMENT_SET_ROTATION: c_uint = 1;
pub const RETRO_ENVIRONMENT_SET_MESSAGE: c_uint = 6;
pub const RETRO_ENVIRONMENT_SHUTDOWN: c_uint = 7;
pub const RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL: c_uint = 8;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: c_uint = 9;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: c_uint = 11;
pub const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: c_uint = 13;
pub const RETRO_ENVIRONMENT_SET_HW_RENDER: c_uint = 14;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: c_uint = 15;
pub const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: c_uint = 17;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: c_uint = 18;
pub const RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE: c_uint = 23;
pub const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: c_uint = 27;
pub const RETRO_ENVIRONMENT_GET_CORE_ASSETS_DIRECTORY: c_uint = 30;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: c_uint = 31;
pub const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: c_uint = 32;
pub const RETRO_ENVIRONMENT_SET_CONTROLLER_INFO: c_uint = 35;
pub const RETRO_ENVIRONMENT_SET_GEOMETRY: c_uint = 37;
pub const RETRO_ENVIRONMENT_GET_USERNAME: c_uint = 38;
pub const RETRO_ENVIRONMENT_GET_LANGUAGE: c_uint = 39;
pub const RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER: c_uint = 40 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE: c_uint = 47 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_FASTFORWARDING: c_uint = 49 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_INPUT_BITMASKS: c_uint = 51 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: c_uint = 52;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY: c_uint = 55;
pub const RETRO_ENVIRONMENT_GET_PREFERRED_HW_RENDER: c_uint = 56;
pub const RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION: c_uint = 57;
pub const RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE: c_uint = 58;
pub const RETRO_ENVIRONMENT_GET_MESSAGE_INTERFACE_VERSION: c_uint = 59;
pub const RETRO_ENVIRONMENT_SET_MESSAGE_EXT: c_uint = 60;
pub const RETRO_ENVIRONMENT_GET_INPUT_MAX_USERS: c_uint = 61;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2: c_uint = 67;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL: c_uint = 68;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK: c_uint = 69;
pub const RETRO_ENVIRONMENT_GET_THROTTLE_STATE: c_uint = 71 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_SAVESTATE_CONTEXT: c_uint = 72 | 0x10000;
pub const RETRO_ENVIRONMENT_GET_VFS_INTERFACE: c_uint = 45 | 0x10000;

// Pixel formats.
pub const RETRO_PIXEL_FORMAT_0RGB1555: c_int = 0;
pub const RETRO_PIXEL_FORMAT_XRGB8888: c_int = 1;
pub const RETRO_PIXEL_FORMAT_RGB565: c_int = 2;

// Devices and button IDs.
pub const RETRO_DEVICE_NONE: c_uint = 0;
pub const RETRO_DEVICE_JOYPAD: c_uint = 1;
pub const RETRO_DEVICE_ANALOG: c_uint = 5;

pub const RETRO_DEVICE_ID_JOYPAD_B: c_uint = 0;
pub const RETRO_DEVICE_ID_JOYPAD_Y: c_uint = 1;
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: c_uint = 2;
pub const RETRO_DEVICE_ID_JOYPAD_START: c_uint = 3;
pub const RETRO_DEVICE_ID_JOYPAD_UP: c_uint = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: c_uint = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: c_uint = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: c_uint = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: c_uint = 8;
pub const RETRO_DEVICE_ID_JOYPAD_X: c_uint = 9;
pub const RETRO_DEVICE_ID_JOYPAD_L: c_uint = 10;
pub const RETRO_DEVICE_ID_JOYPAD_R: c_uint = 11;
pub const RETRO_DEVICE_ID_JOYPAD_L2: c_uint = 12;
pub const RETRO_DEVICE_ID_JOYPAD_R2: c_uint = 13;
pub const RETRO_DEVICE_ID_JOYPAD_L3: c_uint = 14;
pub const RETRO_DEVICE_ID_JOYPAD_R3: c_uint = 15;
pub const RETRO_DEVICE_ID_JOYPAD_MASK: c_uint = 256;

pub const RETRO_DEVICE_INDEX_ANALOG_LEFT: c_uint = 0;
pub const RETRO_DEVICE_INDEX_ANALOG_RIGHT: c_uint = 1;
pub const RETRO_DEVICE_ID_ANALOG_X: c_uint = 0;
pub const RETRO_DEVICE_ID_ANALOG_Y: c_uint = 1;

// Memory maps.
pub const RETRO_MEMORY_SAVE_RAM: c_uint = 0;
pub const RETRO_MEMORY_SYSTEM_RAM: c_uint = 2;

// Audio/video enable flags.
pub const RETRO_AV_ENABLE_VIDEO: c_int = 1;
pub const RETRO_AV_ENABLE_AUDIO: c_int = 2;

// Message targets.
pub const RETRO_MESSAGE_TARGET_ALL: c_int = 0;
pub const RETRO_MESSAGE_TARGET_OSD: c_int = 1;
pub const RETRO_MESSAGE_TARGET_LOG: c_int = 2;

// Language.
pub const RETRO_LANGUAGE_ENGLISH: c_uint = 0;
