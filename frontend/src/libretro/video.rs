use crate::libretro::ffi;
use eframe::egui;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct VideoMailbox {
    frame: Mutex<Option<Arc<Frame>>>,
    seq: AtomicU64,
    ctx: Mutex<Option<egui::Context>>,
    format: AtomicI32,
}

impl VideoMailbox {
    pub fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            seq: AtomicU64::new(0),
            ctx: Mutex::new(None),
            format: AtomicI32::new(ffi::RETRO_PIXEL_FORMAT_RGB565),
        }
    }

    pub fn set_context(&self, ctx: egui::Context) {
        *self.ctx.lock().unwrap() = Some(ctx);
    }

    pub fn set_format(&self, format: i32) -> bool {
        if matches!(
            format,
            ffi::RETRO_PIXEL_FORMAT_0RGB1555
                | ffi::RETRO_PIXEL_FORMAT_XRGB8888
                | ffi::RETRO_PIXEL_FORMAT_RGB565
        ) {
            self.format.store(format, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn latest(&self) -> Option<(Arc<Frame>, u64)> {
        let seq = self.seq.load(Ordering::Acquire);
        self.frame
            .lock()
            .unwrap()
            .as_ref()
            .map(|f| (f.clone(), seq))
    }

    /// Called from the core's video callback. The source pointer is only valid
    /// for the duration of the call, so the frame is copied immediately.
    pub unsafe fn push(&self, data: *const c_void, width: u32, height: u32, pitch: usize) {
        if data.is_null() || width == 0 || height == 0 {
            return;
        }
        let w = width as usize;
        let h = height as usize;
        let mut rgba = vec![0u8; w * h * 4];
        let src = data as *const u8;
        let fmt = self.format.load(Ordering::Relaxed);

        match fmt {
            ffi::RETRO_PIXEL_FORMAT_XRGB8888 => {
                for y in 0..h {
                    let row = src.add(y * pitch) as *const u32;
                    let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                    for x in 0..w {
                        let v = std::ptr::read_unaligned(row.add(x));
                        let r = ((v >> 16) & 0xff) as u8;
                        let g = ((v >> 8) & 0xff) as u8;
                        let b = (v & 0xff) as u8;
                        dst[x * 4] = r;
                        dst[x * 4 + 1] = g;
                        dst[x * 4 + 2] = b;
                        dst[x * 4 + 3] = 255;
                    }
                }
            }
            ffi::RETRO_PIXEL_FORMAT_0RGB1555 => {
                for y in 0..h {
                    let row = src.add(y * pitch) as *const u16;
                    let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                    for x in 0..w {
                        let v = std::ptr::read_unaligned(row.add(x));
                        let r = ((v >> 10) & 0x1f) as u32;
                        let g = ((v >> 5) & 0x1f) as u32;
                        let b = (v & 0x1f) as u32;
                        dst[x * 4] = ((r * 255) / 31) as u8;
                        dst[x * 4 + 1] = ((g * 255) / 31) as u8;
                        dst[x * 4 + 2] = ((b * 255) / 31) as u8;
                        dst[x * 4 + 3] = 255;
                    }
                }
            }
            _ => {
                // RGB565 is the core's default and the common case.
                for y in 0..h {
                    let row = src.add(y * pitch) as *const u16;
                    let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                    for x in 0..w {
                        let v = std::ptr::read_unaligned(row.add(x));
                        let r = ((v >> 11) & 0x1f) as u32;
                        let g = ((v >> 5) & 0x3f) as u32;
                        let b = (v & 0x1f) as u32;
                        dst[x * 4] = ((r * 255) / 31) as u8;
                        dst[x * 4 + 1] = ((g * 255) / 63) as u8;
                        dst[x * 4 + 2] = ((b * 255) / 31) as u8;
                        dst[x * 4 + 3] = 255;
                    }
                }
            }
        }

        *self.frame.lock().unwrap() = Some(Arc::new(Frame {
            width: w,
            height: h,
            rgba,
        }));
        self.seq.fetch_add(1, Ordering::Release);

        if let Some(ctx) = self.ctx.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }
}
