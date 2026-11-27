use crate::libretro::video::Frame;
use anyhow::{anyhow, Context, Result};
use arboard::{Clipboard, ImageData};
use std::borrow::Cow;
use std::sync::{mpsc, Arc, Mutex};

enum ClipboardJob {
    Image(Arc<Frame>),
}

struct ClipboardWorker {
    jobs: mpsc::Sender<ClipboardJob>,
    results: mpsc::Receiver<Result<()>>,
}

impl ClipboardWorker {
    fn spawn() -> Result<Self> {
        let (jobs, job_rx) = mpsc::channel();
        let (results, result_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let mut clipboard = match Clipboard::new() {
                    Ok(clipboard) => clipboard,
                    Err(error) => {
                        let _ = results.send(Err(anyhow!(
                            "could not access the X11/Wayland clipboard: {error}"
                        )));
                        return;
                    }
                };

                while let Ok(job) = job_rx.recv() {
                    let result = match job {
                        ClipboardJob::Image(frame) => copy_image(&mut clipboard, &frame),
                    };
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })
            .context("spawning clipboard worker")?;
        Ok(Self {
            jobs,
            results: result_rx,
        })
    }
}

/// Process-lifetime clipboard worker. Keeping the arboard clipboard object in
/// this thread lets the desktop clipboard manager request image data after the
/// copy call returns, without blocking the UI thread.
#[derive(Default)]
pub struct ClipboardClient {
    worker: Mutex<Option<ClipboardWorker>>,
}

impl ClipboardClient {
    pub fn copy_frame(&self, frame: Arc<Frame>) -> Result<()> {
        validate_frame(&frame)?;
        let mut worker = self.worker.lock().unwrap();
        if worker.is_none() {
            *worker = Some(ClipboardWorker::spawn()?);
        }
        if worker
            .as_ref()
            .expect("clipboard worker initialized")
            .jobs
            .send(ClipboardJob::Image(frame.clone()))
            .is_err()
        {
            // The previous worker exited (for example after a clipboard
            // initialization failure). Start a fresh one and retry once.
            *worker = Some(ClipboardWorker::spawn()?);
            worker
                .as_ref()
                .expect("clipboard worker respawned")
                .jobs
                .send(ClipboardJob::Image(frame))
                .context("queueing clipboard image")?;
        }
        Ok(())
    }

    pub fn poll(&self) -> Option<Result<()>> {
        self.worker
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|worker| worker.results.try_recv().ok())
    }
}

fn copy_image(clipboard: &mut Clipboard, frame: &Frame) -> Result<()> {
    validate_frame(frame)?;
    let image = ImageData {
        width: frame.width,
        height: frame.height,
        bytes: Cow::Borrowed(frame.rgba.as_slice()),
    };
    clipboard
        .set()
        .image(image)
        .map_err(|error| anyhow!("copying image to clipboard: {error}"))
}

fn validate_frame(frame: &Frame) -> Result<()> {
    if frame.width == 0 || frame.height == 0 {
        anyhow::bail!("cannot copy an empty video frame");
    }
    let expected = frame
        .width
        .checked_mul(frame.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("video frame dimensions are too large"))?;
    if frame.rgba.len() < expected {
        anyhow::bail!(
            "video frame has {} RGBA bytes, expected at least {expected}",
            frame.rgba.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: usize, height: usize, len: usize) -> Frame {
        Frame {
            width,
            height,
            rgba: vec![255; len],
        }
    }

    #[test]
    fn validates_frame_dimensions_and_rgba_length() {
        assert!(validate_frame(&frame(2, 2, 16)).is_ok());
        assert!(validate_frame(&frame(0, 2, 0)).is_err());
        assert!(validate_frame(&frame(2, 2, 15)).is_err());
        assert!(validate_frame(&frame(usize::MAX, 2, 0)).is_err());
    }

    #[test]
    fn invalid_frames_are_rejected_before_starting_a_worker() {
        let client = ClipboardClient::default();
        assert!(client.copy_frame(Arc::new(frame(2, 2, 15))).is_err());
        assert!(client.worker.lock().unwrap().is_none());
    }
}
