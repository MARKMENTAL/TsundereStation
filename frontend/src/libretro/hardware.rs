//! Headless OpenGL backend for libretro hardware rendering.
//!
//! SwanStation renders the PS1 GPU into a frontend-provided FBO. We create a
//! small pbuffer-backed EGL context on the emulation thread, give the core a
//! 1024x1024 RGBA8 FBO, then read that FBO back into the existing video
//! mailbox. This keeps the UI path unchanged while moving rasterization off
//! the CPU.

use crate::libretro::ffi;
use crate::libretro::host::{HostState, MessageKind, HOST};
use anyhow::{anyhow, Context, Result};
use glow::HasContext;
use khronos_egl as egl;
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
const FBO_WIDTH: u32 = 1024;
const FBO_HEIGHT: u32 = 1024;

static WARNED: AtomicBool = AtomicBool::new(false);
static PREPARE_FAILED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static HARDWARE: RefCell<Option<Hardware>> = const { RefCell::new(None) };
}

struct Hardware {
    // Field order matters: the EGL instance must outlive the display/context.
    gl: glow::Context,
    fbo: glow::Framebuffer,
    texture: glow::Texture,
    fbo_id: u32,
    context: egl::Context,
    surface: egl::Surface,
    display: egl::Display,
    egl: egl::DynamicInstance<egl::EGL1_4>,
    context_destroy: Option<ffi::retro_hw_context_reset_t>,
}

impl Hardware {
    fn new() -> Result<Self> {
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }
            .map_err(|e| anyhow!("could not load libEGL: {e}"))?;
        let display = init_display(&egl)?;
        egl.bind_api(egl::OPENGL_API)
            .map_err(|e| anyhow!("could not bind the OpenGL EGL API: {e}"))?;

        let config_attrs = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::DEPTH_SIZE,
            0,
            egl::STENCIL_SIZE,
            0,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &config_attrs)
            .context("choosing an EGL config")?
            .ok_or_else(|| anyhow!("no suitable EGL OpenGL config"))?;
        let surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
            .context("creating an EGL pbuffer")?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[
                    egl::CONTEXT_MAJOR_VERSION,
                    3,
                    egl::CONTEXT_MINOR_VERSION,
                    3,
                    egl::CONTEXT_OPENGL_PROFILE_MASK,
                    egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
                    egl::NONE,
                ],
            )
            .context("creating an EGL OpenGL context")?;
        // The core renders into our FBO, so the context does not need a
        // drawable. Surfaceless current also avoids touching the UI thread's
        // X11/Wayland drawable.
        egl.make_current(display, None, None, Some(context))
            .context("making the EGL context current")?;

        let gl = unsafe {
            glow::Context::from_loader_function(|symbol| {
                egl.get_proc_address(symbol)
                    .map(|f| f as *const c_void)
                    .unwrap_or(ptr::null())
            })
        };

        let texture = unsafe { gl.create_texture() }.map_err(|e| anyhow!("GL texture: {e}"))?;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                FBO_WIDTH as i32,
                FBO_HEIGHT as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        let fbo = unsafe { gl.create_framebuffer() }.map_err(|e| anyhow!("GL FBO: {e}"))?;
        let fbo_id = fbo.0.get();
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
                return Err(anyhow!("EGL FBO is incomplete (status {status:#x})"));
            }
        }

        Ok(Self {
            gl,
            fbo,
            texture,
            fbo_id,
            context,
            surface,
            display,
            egl,
            context_destroy: None,
        })
    }

    fn make_current(&self) -> Result<()> {
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(|e| anyhow!("making the EGL context current: {e}"))
    }

    fn read_rgba(&self, width: u32, height: u32) -> Result<(usize, usize, Vec<u8>)> {
        if width == 0 || height == 0 || width > FBO_WIDTH || height > FBO_HEIGHT {
            return Err(anyhow!(
                "hardware frame {width}x{height} exceeds the {}x{} FBO",
                FBO_WIDTH,
                FBO_HEIGHT
            ));
        }
        let w = width as usize;
        let h = height as usize;
        let mut raw = vec![0u8; w * h * 4];
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            self.gl.read_buffer(glow::COLOR_ATTACHMENT0);
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            self.gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut raw)),
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }

        Ok((w, h, flip_and_opaque(&raw, w, h)))
    }
}

fn flip_and_opaque(raw: &[u8], width: usize, height: usize) -> Vec<u8> {
    // The core renders with a bottom-left origin. Flip rows and make the
    // frame opaque for egui's RGBA texture upload.
    let mut rgba = vec![0u8; raw.len()];
    for y in 0..height {
        let src = (height - 1 - y) * width * 4;
        let dst = y * width * 4;
        rgba[dst..dst + width * 4].copy_from_slice(&raw[src..src + width * 4]);
    }
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    rgba
}

// The context is created on the main thread before eframe starts and then
// moved to the worker thread. It is only ever current on one thread at a time.
unsafe impl Send for Hardware {}

impl Drop for Hardware {
    fn drop(&mut self) {
        if let Some(destroy) = self.context_destroy.take() {
            unsafe { destroy() };
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            // GL objects need the context to be current. Release it only
            // after deleting the FBO and texture.
            let _ = self
                .egl
                .make_current(self.display, None, None, Some(self.context));
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.texture);
            let _ = self.egl.make_current(self.display, None, None, None);
            let _ = self.egl.destroy_context(self.display, self.context);
            let _ = self.egl.destroy_surface(self.display, self.surface);
            let _ = self.egl.terminate(self.display);
        }));
    }
}

fn init_display(egl: &egl::DynamicInstance<egl::EGL1_4>) -> Result<egl::Display> {
    // Prefer Mesa's surfaceless platform. It is independent of the UI
    // thread's X11/Wayland context and avoids trying to share the display
    // server connection with eframe.
    if let Some(egl15) = egl.upcast::<egl::EGL1_5>() {
        match unsafe {
            egl15.get_platform_display(
                EGL_PLATFORM_SURFACELESS_MESA,
                ptr::null_mut(),
                &[egl::ATTRIB_NONE],
            )
        } {
            Ok(display) => {
                if egl.initialize(display).is_ok() {
                    log::info!("hardware: using EGL surfaceless display");
                    return Ok(display);
                }
                let _ = egl.terminate(display);
                log::warn!("hardware: EGL surfaceless display failed to initialize");
            }
            Err(e) => log::warn!("hardware: EGL surfaceless platform unavailable: {e:?}"),
        }
    } else {
        log::warn!("hardware: EGL 1.5 upcast unavailable for surfaceless display");
    }

    if let Some(display) = unsafe { egl.get_display(egl::DEFAULT_DISPLAY) } {
        if egl.initialize(display).is_ok() {
            log::info!("hardware: using default EGL display");
            return Ok(display);
        }
        let _ = egl.terminate(display);
        log::warn!("hardware: default EGL display failed to initialize");
    }

    Err(anyhow!("could not initialize an EGL display"))
}

pub fn set_hw_render(cb: &mut ffi::retro_hw_render_callback, host: &HostState) -> bool {
    if !matches!(
        cb.context_type,
        ffi::RETRO_HW_CONTEXT_OPENGL | ffi::RETRO_HW_CONTEXT_OPENGL_CORE
    ) {
        return false;
    }

    if PREPARE_FAILED.load(Ordering::Relaxed) {
        if !WARNED.swap(true, Ordering::Relaxed) {
            host.notify(
                "OpenGL hardware rendering unavailable; using software rendering",
                MessageKind::Warning,
                Duration::from_secs(5),
            );
        }
        return false;
    }

    let result: Result<()> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        HARDWARE.with(|slot| {
            if slot.borrow().is_none() {
                let hardware = Hardware::new()?;
                *slot.borrow_mut() = Some(hardware);
            }
            let mut slot = slot.borrow_mut();
            let hardware = slot.as_mut().expect("hardware initialized above");
            hardware.context_destroy = cb.context_destroy;
            Ok(())
        })
    }))
    .unwrap_or_else(|_| Err(anyhow!("an EGL/OpenGL operation panicked")));

    if let Err(e) = result {
        log::warn!("Could not initialise OpenGL hardware rendering: {e:#}");
        if !WARNED.swap(true, Ordering::Relaxed) {
            host.notify(
                "OpenGL hardware rendering unavailable; using software rendering",
                MessageKind::Warning,
                Duration::from_secs(5),
            );
        }
        return false;
    }

    cb.get_current_framebuffer = Some(get_current_framebuffer);
    cb.get_proc_address = Some(get_proc_address);
    cb.depth = false;
    cb.stencil = false;
    cb.bottom_left_origin = true;
    cb.cache_context = false;

    host.hardware_available.store(true, Ordering::Relaxed);
    host.hardware_active.store(false, Ordering::Relaxed);
    WARNED.store(false, Ordering::Relaxed);
    log::info!("hardware: OpenGL renderer initialised");

    // Do not hold a borrow of HARDWARE here: the core's context_reset calls
    // get_proc_address/get_current_framebuffer back into this module.
    if let Some(reset) = cb.context_reset {
        unsafe { reset() };
    }
    true
}

/// Prepare the EGL/OpenGL context on the emulation worker before eframe
/// creates its UI context. The context stays current on the worker.
pub fn prepare_on_worker() {
    let result: Result<()> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        HARDWARE.with(|slot| {
            if slot.borrow().is_none() {
                let hardware = Hardware::new()?;
                *slot.borrow_mut() = Some(hardware);
            }
            Ok(())
        })
    }))
    .unwrap_or_else(|_| Err(anyhow!("an EGL/OpenGL operation panicked")));

    match result {
        Ok(()) => {
            PREPARE_FAILED.store(false, Ordering::Relaxed);
            WARNED.store(false, Ordering::Relaxed);
            if let Some(host) = HOST.get() {
                host.hardware_available.store(true, Ordering::Relaxed);
            }
            log::info!("hardware: prepared headless EGL context on the emulation worker");
        }
        Err(e) => {
            PREPARE_FAILED.store(true, Ordering::Relaxed);
            WARNED.store(true, Ordering::Relaxed);
            log::warn!("Could not prepare the EGL context: {e:#}");
            if let Some(host) = HOST.get() {
                host.notify(
                    "OpenGL hardware rendering unavailable; using software rendering",
                    MessageKind::Warning,
                    Duration::from_secs(5),
                );
            }
        }
    }
}

pub fn read_frame(width: u32, height: u32) -> bool {
    let frame = HARDWARE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|hardware| hardware.read_rgba(width, height).ok())
    });
    let Some((w, h, rgba)) = frame else {
        return false;
    };
    if let Some(host) = HOST.get() {
        host.video.push_rgba(w, h, rgba);
        if !host.hardware_active.swap(true, Ordering::Relaxed) {
            log::info!("hardware: first hardware-rendered frame");
        }
    }
    true
}

pub fn release_context(core_is_loaded: bool) {
    // The core's context_destroy switches its GPU back to software. That is
    // only valid while the emulated system is still loaded.
    let destroy = if core_is_loaded {
        HARDWARE.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|hardware| hardware.context_destroy)
        })
    } else {
        None
    };
    if let Some(destroy) = destroy {
        unsafe { destroy() };
    }

    // Keep the EGL context/FBO alive on the worker for the next core load.
    // It was prepared before eframe started and must not be recreated while
    // the UI GL context is live.
    let available = HARDWARE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(hardware) = slot.as_mut() {
            hardware.context_destroy = None;
            if let Err(e) = hardware.make_current() {
                log::warn!("Could not restore the worker EGL context: {e:#}");
            }
            true
        } else {
            false
        }
    });

    if let Some(host) = HOST.get() {
        host.hardware_active.store(false, Ordering::Relaxed);
        host.hardware_available.store(available, Ordering::Relaxed);
    }
}

pub fn is_hardware_frame(data: *const c_void) -> bool {
    data as isize == ffi::RETRO_HW_FRAME_BUFFER_VALID
}

unsafe extern "C" fn get_current_framebuffer() -> usize {
    HARDWARE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|hardware| hardware.fbo_id as usize)
            .unwrap_or(0)
    })
}

unsafe extern "C" fn get_proc_address(symbol: *const c_char) -> *mut c_void {
    if symbol.is_null() {
        return ptr::null_mut();
    }
    let name = CStr::from_ptr(symbol).to_string_lossy();
    HARDWARE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|hardware| hardware.egl.get_proc_address(&name))
            .map(|f| f as *const () as *mut c_void)
            .unwrap_or(ptr::null_mut())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_sentinel_is_recognized() {
        assert!(is_hardware_frame(
            ffi::RETRO_HW_FRAME_BUFFER_VALID as *const c_void
        ));
        assert!(!is_hardware_frame(ptr::null()));
        assert!(!is_hardware_frame(0x1000 as *const c_void));
    }

    #[test]
    fn readback_flips_rows_and_forces_opaque_alpha() {
        // Two rows, two pixels: bottom row is red, top row is green.
        let raw = [
            255, 0, 0, 0, 255, 0, 0, 0, // bottom (read first)
            0, 255, 0, 0, 0, 255, 0, 0, // top
        ];
        let flipped = flip_and_opaque(&raw, 2, 2);
        assert_eq!(&flipped[0..4], &[0, 255, 0, 255]);
        assert_eq!(&flipped[4..8], &[0, 255, 0, 255]);
        assert_eq!(&flipped[8..12], &[255, 0, 0, 255]);
        assert_eq!(&flipped[12..16], &[255, 0, 0, 255]);
    }
}
