mod app;
mod config;
mod emu;
mod filebrowser;
mod libretro;
mod memory_card;
mod screenshot;
mod sdl_input;
mod splash;
mod theme;
mod tv_ui;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let mut config = config::Config::load();
    let mut selftest = false;
    let mut auto_start = false;
    let mut auto_boot_bios = false;
    let mut splash_override: Option<bool> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--core" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    config.core_path = PathBuf::from(p);
                }
            }
            "--game" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    config.last_game = p.clone();
                    auto_start = true;
                }
            }
            "--boot-bios" => auto_boot_bios = true,
            "--bios-dir" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    config.bios_dir = PathBuf::from(p);
                }
            }
            "--no-splash" => splash_override = Some(false),
            "--splash" => splash_override = Some(true),
            "--selftest" => selftest = true,
            "--version" | "-V" => {
                println!("TsundereStation {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "tsunderestation [--core <core.so>] [--game <disc>] [--boot-bios] [--bios-dir <dir>] [--no-splash] [--splash] [--selftest] [--version]"
                );
                return Ok(());
            }
            other => log::warn!("Ignoring unknown argument {other}"),
        }
        i += 1;
    }

    if selftest {
        let temp = std::env::temp_dir().join("tsunderestation-selftest");
        config.bios_dir = temp.join("bios");
        config.save_dir = temp.join("saves");
        config.state_dir = temp.join("states");
    }

    config.ensure_dirs();

    let core_path = config.resolve_core();
    let host = Arc::new(libretro::host::HostState::new(
        &config.bios_dir,
        &config.save_dir,
    ));
    if libretro::host::HOST.set(host.clone()).is_err() {
        anyhow::bail!("host state was already initialized");
    }

    if selftest {
        return run_selftest(&config, core_path, host);
    }

    let core = match core_path {
        Some(p) => p,
        None => {
            anyhow::bail!(
                "Could not find swanstation_libretro.so. Build it with `make -f Makefile.libretro platform=unix` \
                 or pass --core /path/to/swanstation_libretro.so"
            );
        }
    };

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1000.0, 640.0])
        .with_title("TsundereStation")
        .with_app_id("tsunderestation");
    match splash::window_icon() {
        Ok(icon) => viewport = viewport.with_icon(Arc::new(icon)),
        Err(e) => log::warn!("Could not decode the window icon: {e:#}"),
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "TsundereStation",
        native_options,
        Box::new(move |cc| {
            let app = app::App::new(
                cc,
                config,
                host,
                core,
                auto_start,
                auto_boot_bios,
                splash_override,
            )
            .context("initializing frontend")?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}

fn run_selftest(
    config: &config::Config,
    core_path: Option<PathBuf>,
    host: Arc<libretro::host::HostState>,
) -> Result<()> {
    let core = core_path.context("core library not found")?;
    log::info!("Self-test using core {}", core.display());

    let mut ch = libretro::loader::CoreHost::new(&core, host.clone(), &config.core_options)?;
    let info = ch.info().clone();
    println!("core: {} {}", info.name, info.version);
    println!("core options: {}", info.options.len());
    for (i, port) in info.ports.iter().enumerate() {
        for (name, id) in &port.types {
            println!("port {i}: {name} ({id:#x})");
        }
    }

    let port = info
        .ports
        .first()
        .and_then(|p| {
            p.types
                .iter()
                .find(|(n, _)| n.contains("DualShock"))
                .or_else(|| p.types.first())
        })
        .map(|(_, id)| *id);

    host.audio.set_enabled(true);
    ch.load_game(None, &[port, None])?;
    println!(
        "booted BIOS-only content: {}x{} @ {:.3} fps",
        host.base_width.load(std::sync::atomic::Ordering::Relaxed),
        host.base_height.load(std::sync::atomic::Ordering::Relaxed),
        host.fps()
    );

    for _ in 0..120 {
        ch.run_frame();
    }

    if let Some((frame, _)) = host.video.latest() {
        println!("frame: {}x{}", frame.width, frame.height);
        let ppm_path = std::env::temp_dir().join("tsunderestation-selftest.ppm");
        let mut ppm = Vec::with_capacity(32 + frame.rgba.len() / 4 * 3);
        ppm.extend_from_slice(format!("P6\n{} {}\n255\n", frame.width, frame.height).as_bytes());
        for px in frame.rgba.chunks_exact(4) {
            ppm.extend_from_slice(&px[0..3]);
        }
        std::fs::write(&ppm_path, ppm)?;
        println!("wrote {}", ppm_path.display());
    } else {
        anyhow::bail!("core produced no video frames");
    }
    println!("audio frames queued: {}", host.audio.queued_frames());

    let state = ch.save_state()?;
    println!("save state size: {}", state.len());
    let tmp = std::env::temp_dir().join("tsunderestation-selftest.state");
    std::fs::write(&tmp, &state)?;
    ch.load_state_from(&tmp)?;
    println!("state round-trip ok");
    ch.save_memory_card();
    ch.shutdown();
    println!("selftest ok");
    Ok(())
}
