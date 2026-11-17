use anyhow::{bail, Context, Result};
use eframe::egui;
use minimp3::Decoder;
use std::io::Cursor;
use std::time::{Duration, Instant};

pub const LOGO_JPG: &[u8] = include_bytes!("../assets/tsunderestation-logo.jpg");
pub const MOTIF_MP3: &[u8] = include_bytes!("../assets/tsunderestation-splash.mp3");

/// Embedded logo as an egui image, used for the launcher header and splash.
pub fn logo_image() -> Result<egui::ColorImage> {
    let image = image::load_from_memory(LOGO_JPG).context("decoding logo JPEG")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        &rgba.into_raw(),
    ))
}

/// Embedded logo as a native window icon.
pub fn window_icon() -> Result<egui::IconData> {
    let image = image::load_from_memory(LOGO_JPG).context("decoding logo JPEG")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

/// The splash motif, decoded to interleaved stereo f32 at 44.1 kHz.
pub struct SplashAudio {
    samples: Vec<f32>,
    pos: usize,
    fade_start: Option<Instant>,
    fade_len: Duration,
    stopped: bool,
}

impl SplashAudio {
    pub fn empty() -> Self {
        Self {
            samples: Vec::new(),
            pos: 0,
            fade_start: None,
            fade_len: Duration::ZERO,
            stopped: true,
        }
    }

    pub fn decode_motif() -> Result<Self> {
        let mut decoder = Decoder::new(Cursor::new(MOTIF_MP3));
        let mut raw = Vec::<f32>::new();
        let mut sample_rate = 44100u32;
        let mut channels = 2usize;

        while let Ok(frame) = decoder.next_frame() {
            sample_rate = frame.sample_rate.max(1) as u32;
            channels = frame.channels.max(1);
            match channels {
                1 => {
                    for sample in &frame.data {
                        let s = *sample as f32 / 32768.0;
                        raw.push(s);
                        raw.push(s);
                    }
                }
                _ => {
                    for frame in frame.data.chunks(channels) {
                        if frame.len() >= 2 {
                            raw.push(frame[0] as f32 / 32768.0);
                            raw.push(frame[1] as f32 / 32768.0);
                        }
                    }
                }
            }
        }

        if raw.is_empty() {
            bail!("splash motif decoded to no samples");
        }

        let mut samples = if sample_rate != 44100 {
            resample_stereo(&raw, sample_rate, 44100)
        } else {
            raw
        };
        // Leave a little headroom for the emulator's own audio when mixing.
        for sample in &mut samples {
            *sample *= 0.8;
        }

        log::info!(
            "Decoded splash motif: {:.2}s at {} Hz ({} source channels)",
            samples.len() as f64 / 2.0 / 44100.0,
            sample_rate,
            channels
        );

        Ok(Self {
            samples,
            pos: 0,
            fade_start: None,
            fade_len: Duration::ZERO,
            stopped: false,
        })
    }

    pub fn fade_out(&mut self, len: Duration) {
        if self.stopped {
            return;
        }
        self.fade_start = Some(Instant::now());
        self.fade_len = len;
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.pos = self.samples.len();
        self.fade_start = None;
    }

    pub fn is_finished(&self) -> bool {
        self.stopped || self.pos >= self.samples.len()
    }

    /// Return the next interleaved stereo sample.
    pub fn next_sample(&mut self) -> f32 {
        if self.is_finished() {
            return 0.0;
        }
        let mut gain = 1.0f32;
        if let Some(start) = self.fade_start {
            let t = start.elapsed().as_secs_f32() / self.fade_len.as_secs_f32().max(0.001);
            if t >= 1.0 {
                self.stop();
                return 0.0;
            }
            gain = 1.0 - t;
        }
        let sample = self.samples[self.pos];
        self.pos += 1;
        sample * gain
    }
}

fn resample_stereo(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let src_frames = input.len() / 2;
    let dst_frames = ((src_frames as f64) * (dst_rate as f64) / (src_rate as f64)).round() as usize;
    let mut out = Vec::with_capacity(dst_frames.max(1) * 2);
    for frame in 0..dst_frames.max(1) {
        let src_pos = frame as f64 * src_rate as f64 / dst_rate as f64;
        let i0 = (src_pos.floor() as usize).min(src_frames.saturating_sub(1));
        let i1 = (i0 + 1).min(src_frames.saturating_sub(1));
        let frac = (src_pos - i0 as f64) as f32;
        for channel in 0..2 {
            let a = input[i0 * 2 + channel];
            let b = input[i1 * 2 + channel];
            out.push(a + (b - a) * frac);
        }
    }
    out
}
