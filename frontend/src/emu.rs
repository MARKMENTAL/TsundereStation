use crate::libretro::host::{HostState, MessageKind};
use crate::libretro::loader::{state_path, CoreHost, CoreInfo};
use anyhow::Result;
use std::collections::BTreeMap;
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
            .name("swanstation-core".to_string())
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
                            next_frame = Instant::now();
                        }
                        Err(e) => {
                            let msg = format!("Could not load game: {e:#}");
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
                            next_frame = Instant::now();
                        }
                        Err(e) => {
                            let msg = format!("Could not boot BIOS: {e:#}");
                            let _ = events.send(WorkerEvent::Error(msg));
                        }
                    }
                }
                Command::UnloadGame => {
                    if let Err(e) = core.unload_game() {
                        log::warn!("unload failed: {e:#}");
                    }
                    paused = false;
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
                            }
                            Err(e) => {
                                let msg = format!("Core restarted, but game reload failed: {e:#}");
                                let _ = events.send(WorkerEvent::Error(msg));
                            }
                        }
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
