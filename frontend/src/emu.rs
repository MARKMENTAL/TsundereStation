use crate::libretro::host::{HostState, MessageKind};
use crate::libretro::loader::{state_path, CoreHost, CoreInfo};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub enum Command {
    LoadGame {
        path: String,
        port_device: Option<u32>,
    },
    BootBios {
        port_device: Option<u32>,
    },
    UnloadGame,
    Pause(bool),
    Reset,
    SaveState(u32),
    LoadState(u32),
    SetPortDevice(u32, u32),
    SwapDisc(usize),
    RestartCore,
    Quit,
}

#[derive(Clone, Debug)]
pub enum WorkerEvent {
    Ready(CoreInfo),
    Booted { title: String, path: String },
    Unloaded,
    StateSaved(u32),
    StateLoaded(u32),
    DiscInfo { labels: Vec<String>, current: usize },
    Error(String),
    Stopped,
}

pub struct WorkerHandle {
    tx: Sender<Command>,
    events: Receiver<WorkerEvent>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn spawn(
        core_path: PathBuf,
        host: Arc<HostState>,
        saved_options: BTreeMap<String, String>,
        state_dir: PathBuf,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Command>();
        let (event_tx, events) = mpsc::channel::<WorkerEvent>();

        let join = thread::Builder::new()
            .name("tsunderestation-core".to_string())
            .spawn(move || {
                let result = run_worker(core_path, host, saved_options, state_dir, rx, &event_tx);
                if let Err(e) = result {
                    log::error!("core worker failed: {e:#}");
                    let _ = event_tx.send(WorkerEvent::Error(format!("{e:#}")));
                }
                let _ = event_tx.send(WorkerEvent::Stopped);
            })
            .expect("spawning core worker thread");

        Self {
            tx,
            events,
            join: Some(join),
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv_events(&self) -> Vec<WorkerEvent> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn buffer_string(buffer: &[u8]) -> Option<String> {
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn query_disc_info(host: &HostState) -> Option<(Vec<String>, usize)> {
    let control = host.disk_control()?;
    let get_num_images = control.get_num_images?;
    let get_image_index = control.get_image_index?;
    let count = get_num_images() as usize;
    if count == 0 {
        return Some((Vec::new(), 0));
    }

    let current = get_image_index() as usize;
    let mut labels = Vec::with_capacity(count);
    for index in 0..count {
        let mut buffer = vec![0u8; 4096];
        let mut ok = false;
        if let Some(get_label) = control.get_image_label {
            ok = get_label(
                index as u32,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len(),
            );
        }
        if !ok {
            if let Some(get_path) = control.get_image_path {
                ok = get_path(
                    index as u32,
                    buffer.as_mut_ptr() as *mut c_char,
                    buffer.len(),
                );
            }
        }
        labels.push(if ok {
            buffer_string(&buffer).unwrap_or_else(|| format!("Disc {}", index + 1))
        } else {
            format!("Disc {}", index + 1)
        });
    }

    Some((labels, current.min(count - 1)))
}

fn send_disc_info(host: &HostState, events: &Sender<WorkerEvent>) {
    let (labels, current) = query_disc_info(host).unwrap_or_default();
    let _ = events.send(WorkerEvent::DiscInfo { labels, current });
}

fn send_empty_disc_info(events: &Sender<WorkerEvent>) {
    let _ = events.send(WorkerEvent::DiscInfo {
        labels: Vec::new(),
        current: 0,
    });
}

fn swap_disc(host: &HostState, index: usize) -> Result<()> {
    let control = host
        .disk_control()
        .context("core has no disk control interface")?;
    let get_num_images = control
        .get_num_images
        .context("core has no image count callback")?;
    let set_eject_state = control
        .set_eject_state
        .context("core cannot eject its disc tray")?;
    let set_image_index = control
        .set_image_index
        .context("core cannot select a disc image")?;

    let count = get_num_images() as usize;
    if index >= count {
        bail!("disc index {} is out of range (0..{})", index + 1, count);
    }
    if !set_eject_state(true) {
        bail!("core refused to open the disc tray");
    }
    if !set_image_index(index as u32) {
        let _ = set_eject_state(false);
        bail!("core refused to insert disc {}", index + 1);
    }
    if !set_eject_state(false) {
        bail!("core refused to close the disc tray");
    }
    Ok(())
}

fn run_worker(
    core_path: PathBuf,
    host: Arc<HostState>,
    saved_options: BTreeMap<String, String>,
    state_dir: PathBuf,
    rx: Receiver<Command>,
    events: &Sender<WorkerEvent>,
) -> Result<()> {
    let mut core = CoreHost::new(&core_path, host.clone(), &saved_options)?;
    let _ = events.send(WorkerEvent::Ready(core.info().clone()));

    let mut paused = false;
    let mut quit = false;
    let mut next_frame = Instant::now();
    let mut last_memcard_save = Instant::now();
    let mut current_port_device: Option<u32> = None;

    while !quit {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Command::LoadGame { path, port_device } => {
                    if port_device.is_some() {
                        current_port_device = port_device;
                    }
                    let p = PathBuf::from(&path);
                    match core.load_game(Some(&p), port_device) {
                        Ok(()) => {
                            paused = false;
                            let title = p
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.clone());
                            host.notify(
                                format!("Loaded {title}"),
                                MessageKind::Info,
                                Duration::from_secs(2),
                            );
                            let _ = events.send(WorkerEvent::Booted { title, path });
                            send_disc_info(&host, events);
                            next_frame = Instant::now();
                        }
                        Err(e) => {
                            let msg = format!("Could not load game: {e:#}");
                            send_empty_disc_info(events);
                            let _ = events.send(WorkerEvent::Error(msg));
                        }
                    }
                }
                Command::BootBios { port_device } => {
                    if port_device.is_some() {
                        current_port_device = port_device;
                    }
                    match core.load_game(None, port_device) {
                        Ok(()) => {
                            paused = false;
                            host.notify("Booted BIOS", MessageKind::Info, Duration::from_secs(2));
                            let _ = events.send(WorkerEvent::Booted {
                                title: "BIOS".to_string(),
                                path: String::new(),
                            });
                            send_empty_disc_info(events);
                            next_frame = Instant::now();
                        }
                        Err(e) => {
                            let msg = format!("Could not boot BIOS: {e:#}");
                            send_empty_disc_info(events);
                            let _ = events.send(WorkerEvent::Error(msg));
                        }
                    }
                }
                Command::UnloadGame => {
                    if let Err(e) = core.unload_game() {
                        log::warn!("unload failed: {e:#}");
                    }
                    paused = false;
                    send_empty_disc_info(events);
                    let _ = events.send(WorkerEvent::Unloaded);
                }
                Command::Pause(value) => paused = value,
                Command::Reset => {
                    core.reset();
                    host.notify("Console reset", MessageKind::Info, Duration::from_secs(2));
                }
                Command::SaveState(slot) => {
                    let game = core
                        .game()
                        .map(|g| g.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("bios"));
                    let path = state_path(&state_dir, &game, slot);
                    match core.save_state_to(&path) {
                        Ok(()) => {
                            let _ = events.send(WorkerEvent::StateSaved(slot));
                        }
                        Err(e) => {
                            let msg = format!("Save state {slot} failed: {e:#}");
                            let _ = events.send(WorkerEvent::Error(msg));
                        }
                    }
                }
                Command::LoadState(slot) => {
                    let game = core
                        .game()
                        .map(|g| g.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("bios"));
                    let path = state_path(&state_dir, &game, slot);
                    match core.load_state_from(&path) {
                        Ok(()) => {
                            let _ = events.send(WorkerEvent::StateLoaded(slot));
                            next_frame = Instant::now();
                        }
                        Err(e) => {
                            let msg = format!("Load state {slot} failed: {e:#}");
                            let _ = events.send(WorkerEvent::Error(msg));
                        }
                    }
                }
                Command::SetPortDevice(port, device) => {
                    core.set_port_device(port, device);
                    if port == 0 {
                        current_port_device = Some(device);
                    }
                }
                Command::SwapDisc(index) => match swap_disc(&host, index) {
                    Ok(()) => {
                        send_disc_info(&host, events);
                        host.notify(
                            format!("Inserted disc {}", index + 1),
                            MessageKind::Info,
                            Duration::from_secs(2),
                        );
                        next_frame = Instant::now();
                    }
                    Err(e) => {
                        let msg = format!("Disc swap failed: {e:#}");
                        let _ = events.send(WorkerEvent::Error(msg));
                    }
                },
                Command::RestartCore => {
                    let previous_game = core.game().map(|p| p.to_path_buf());
                    core.shutdown();
                    let overrides: BTreeMap<String, String> = host
                        .options
                        .read()
                        .unwrap()
                        .overrides
                        .clone()
                        .into_iter()
                        .collect();
                    core = CoreHost::new(&core_path, host.clone(), &overrides)?;
                    let _ = events.send(WorkerEvent::Ready(core.info().clone()));
                    if let Some(game) = previous_game {
                        match core.load_game(Some(&game), current_port_device) {
                            Ok(()) => {
                                paused = false;
                                let title = game
                                    .file_name()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| game.display().to_string());
                                let _ = events.send(WorkerEvent::Booted {
                                    title,
                                    path: game.to_string_lossy().to_string(),
                                });
                                send_disc_info(&host, events);
                            }
                            Err(e) => {
                                let msg = format!("Core restarted, but game reload failed: {e:#}");
                                send_empty_disc_info(events);
                                let _ = events.send(WorkerEvent::Error(msg));
                            }
                        }
                    } else {
                        send_empty_disc_info(events);
                    }
                    next_frame = Instant::now();
                }
                Command::Quit => quit = true,
            }
        }

        if quit {
            break;
        }

        if paused {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        if !core.is_loaded() {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        let fps = host.fps();
        let frame_time = Duration::from_secs_f64(1.0 / fps);
        let fast_forward = host.fast_forward.load(Ordering::Relaxed);
        let frames = if fast_forward { 4 } else { 1 };
        for _ in 0..frames {
            core.run_frame();
        }

        if last_memcard_save.elapsed() > Duration::from_secs(30) {
            core.save_memory_card();
            last_memcard_save = Instant::now();
        }

        next_frame += frame_time;
        let now = Instant::now();
        if next_frame > now {
            thread::sleep(next_frame - now);
        } else {
            next_frame = now;
        }

        // Safety valve for the audio queue. Normally frame pacing is enough.
        if !fast_forward && host.audio.is_enabled() {
            let mut guard = 0;
            while host.audio.queued_frames() > 4096 && guard < 20 {
                thread::sleep(Duration::from_millis(1));
                guard += 1;
            }
        }
    }

    core.save_memory_card();
    core.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libretro::ffi;
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Mutex;

    static EJECTED: AtomicBool = AtomicBool::new(false);
    static IMAGE_INDEX: AtomicUsize = AtomicUsize::new(0);
    static FAIL_SELECT: AtomicBool = AtomicBool::new(false);
    static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn mock_num_images() -> u32 {
        3
    }

    extern "C" fn mock_get_image_index() -> u32 {
        IMAGE_INDEX.load(Ordering::SeqCst) as u32
    }

    extern "C" fn mock_get_eject_state() -> bool {
        EJECTED.load(Ordering::SeqCst)
    }

    extern "C" fn mock_set_eject_state(ejected: bool) -> bool {
        EJECTED.store(ejected, Ordering::SeqCst);
        CALLS
            .lock()
            .unwrap()
            .push(if ejected { "eject" } else { "close" }.to_string());
        true
    }

    extern "C" fn mock_set_image_index(index: u32) -> bool {
        if !EJECTED.load(Ordering::SeqCst) || FAIL_SELECT.load(Ordering::SeqCst) {
            return false;
        }
        IMAGE_INDEX.store(index as usize, Ordering::SeqCst);
        CALLS.lock().unwrap().push(format!("select {}", index + 1));
        true
    }

    extern "C" fn mock_get_image_label(index: u32, buffer: *mut c_char, len: usize) -> bool {
        let value = format!("Disc {}", index + 1);
        let bytes = value.as_bytes();
        if buffer.is_null() || len <= bytes.len() {
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, bytes.len());
            *buffer.add(bytes.len()) = 0;
        }
        true
    }

    fn mock_control() -> ffi::retro_disk_control_ext_callback {
        ffi::retro_disk_control_ext_callback {
            set_eject_state: Some(mock_set_eject_state),
            get_eject_state: Some(mock_get_eject_state),
            get_image_index: Some(mock_get_image_index),
            set_image_index: Some(mock_set_image_index),
            get_num_images: Some(mock_num_images),
            replace_image_index: None,
            add_image_index: None,
            set_initial_image: None,
            get_image_path: None,
            get_image_label: Some(mock_get_image_label),
        }
    }

    #[test]
    fn disc_swap_uses_eject_select_close_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = HostState::new(dir.path(), dir.path());
        let control = mock_control();
        host.set_disk_control_ext(&control);
        EJECTED.store(false, Ordering::SeqCst);
        IMAGE_INDEX.store(0, Ordering::SeqCst);
        FAIL_SELECT.store(false, Ordering::SeqCst);
        CALLS.lock().unwrap().clear();

        swap_disc(&host, 2).expect("swaps");
        assert_eq!(IMAGE_INDEX.load(Ordering::SeqCst), 2);
        assert_eq!(
            *CALLS.lock().unwrap(),
            vec![
                "eject".to_string(),
                "select 3".to_string(),
                "close".to_string()
            ]
        );
        let (labels, current) = query_disc_info(&host).expect("disc info");
        assert_eq!(labels, vec!["Disc 1", "Disc 2", "Disc 3"]);
        assert_eq!(current, 2);

        CALLS.lock().unwrap().clear();
        FAIL_SELECT.store(true, Ordering::SeqCst);
        assert!(swap_disc(&host, 1).is_err());
        assert_eq!(
            *CALLS.lock().unwrap(),
            vec!["eject".to_string(), "close".to_string()]
        );
    }
}
