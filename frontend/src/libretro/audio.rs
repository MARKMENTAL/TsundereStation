use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Interleaved stereo f32 samples shared between the emulation thread and the
/// SDL audio callback. The queue is intentionally bounded: if the emulator
/// outruns the audio device we drop the oldest samples rather than growing
/// without limit.
pub struct AudioRing {
    buf: Mutex<VecDeque<f32>>,
    max_samples: usize,
    enabled: AtomicBool,
}

impl AudioRing {
    pub fn new() -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(16 * 1024)),
            max_samples: 32 * 1024,
            enabled: AtomicBool::new(false),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn push_interleaved_i16(&self, data: &[i16]) {
        if !self.is_enabled() {
            return;
        }
        let mut buf = self.buf.lock().unwrap();
        if buf.len() + data.len() > self.max_samples {
            let drop = (buf.len() + data.len()).saturating_sub(self.max_samples);
            for _ in 0..drop.min(buf.len()) {
                buf.pop_front();
            }
        }
        buf.extend(data.iter().map(|&s| s as f32 / 32768.0));
    }

    pub fn push_stereo(&self, left: i16, right: i16) {
        if !self.is_enabled() {
            return;
        }
        let mut buf = self.buf.lock().unwrap();
        if buf.len() + 2 > self.max_samples {
            buf.pop_front();
            buf.pop_front();
        }
        buf.push_back(left as f32 / 32768.0);
        buf.push_back(right as f32 / 32768.0);
    }

    pub fn fill(&self, out: &mut [f32]) {
        let mut buf = self.buf.lock().unwrap();
        for sample in out.iter_mut() {
            *sample = buf.pop_front().unwrap_or(0.0);
        }
    }

    pub fn queued_frames(&self) -> usize {
        self.buf.lock().unwrap().len() / 2
    }
}
