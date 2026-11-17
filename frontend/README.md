# TsundereStation

A small Linux-only native frontend for the SwanStation libretro core, built with
Rust, egui/eframe and SDL2. It loads `swanstation_libretro.so` directly; RetroArch
is not required.

The app includes the TsundereStation logo and a short startup splash with the
provided motif. Both assets are embedded in the binary.

## Build

Build the core from the repository root first:

```sh
make -f Makefile.libretro platform=unix -j"$(nproc)"
```

Then build the frontend:

```sh
cd frontend
cargo build --release
```

The default build uses the system SDL2 development package (for example
`libsdl2-dev` on Debian/Ubuntu) and an OpenGL + X11/Wayland toolchain. If you
would rather compile SDL2 from source:

```sh
cargo build --release --features bundled-sdl
```

## Run

```sh
./target/release/tsunderestation
```

Useful flags:

```sh
# Boot directly into a disc image
./target/release/tsunderestation --game /path/to/game.cue

# Boot the BIOS shell (memory-card manager, BIOS test)
./target/release/tsunderestation --boot-bios

# Skip the startup splash for this run
./target/release/tsunderestation --no-splash

# Choose a different core library or BIOS directory
./target/release/tsunderestation --core /path/to/swanstation_libretro.so \
    --bios-dir /path/to/bios

# Headless smoke test: boot BIOS, run frames, save/load state, dump a frame
./target/release/tsunderestation --selftest
```

The splash is shown for about four seconds and is skipped by any keyboard key,
mouse click, or controller button. It can be disabled with `--no-splash` or the
“Show splash on startup” checkbox in the launcher. `splash_seconds` in the
config file controls the duration.

## Data locations

Following the XDG base directory spec:

- Config: `~/.config/tsunderestation/config.toml`
- BIOS images: `~/.local/share/tsunderestation/bios/`
- Memory cards: `~/.local/share/tsunderestation/saves/*.srm`
- Save states: `~/.local/share/tsunderestation/states/*.state`

On first run, an existing `swanstation-ui` config and data directory are migrated
to the new `tsunderestation` paths automatically. The data directory is moved when
possible; the old config file is left in place.

BIOS files are optional. If the BIOS folder is empty, the core uses its built-in
OpenBIOS. Use the launcher's “Add BIOS image…” button to copy a `.bin`, `.rom`,
or `.img` image into the managed BIOS folder; the core selects a region-appropriate
image on boot.

## Controls

Keyboard:

- `Esc` / `F1`: open/close the in-game menu
- `F5`: quick save (slot 0)
- `F8`: quick load (slot 0)
- `F11`: toggle fullscreen
- Hold `Tab`: fast-forward (audio muted, video shown)
- In the menu: arrow keys move, `Enter` selects, `Esc` closes

SDL game controller:

- `PS` / `Guide` button: open/close the in-game menu
- `Select + Start`: optional fallback menu combo (enable in Controller settings)
- In the menu: D-pad/left stick moves, Cross selects, Circle closes
- Left/Right changes the save-state slot
- The standard SDL gamepad mapping is used: Cross → PS1 Cross, Circle → Circle,
  Square → Square, Triangle → Triangle, shoulders → L1/R1, triggers → L2/R2,
  stick clicks → L3/R3.

If the PS button opens Steam's overlay instead of this menu, disable Steam Input
for the controller (or launch the frontend outside Steam). SDL's HIDAPI driver is
enabled by the frontend; if the controller is not detected, check hidraw/udev
permissions.

## Desktop install

The `install.sh` helper installs the frontend, the desktop entry, the icon, and
the built core under `~/.local`:

```sh
./install.sh
```

It copies:

- `~/.local/bin/tsunderestation`
- `~/.local/share/applications/tsunderestation.desktop`
- `~/.local/share/icons/hicolor/512x512/apps/tsunderestation.png`
- `~/.local/lib/libretro/swanstation_libretro.so` (if the core is built)

Make sure `~/.local/bin` is in your `PATH`. The frontend also searches for the
core next to the executable, in the current directory, in
`~/.local/lib/libretro`, and in `/usr/lib/libretro`; or set
`TSUNDERESTATION_CORE` / use `--core`.

## Current limitations

- Software renderer only. Hardware OpenGL/Vulkan rendering, internal-resolution
  upscaling and PGXP are not wired up yet.
- One controller is mapped to port 1; multitap and multiple controllers are not
  exposed yet.
- Multi-disc swapping is not exposed yet (single-disc images, and the first disc
  of an `.m3u`, work).
- Rewind, runahead and cheats are not exposed yet.
- Core options mostly apply on the next frame. When no game is running, changing
  an option automatically reinitializes the core so boot-time options apply to
  the next start. While a game is running, use the Settings window's “Restart
  core” button (the current game is reloaded automatically).
- Audio output uses SDL's default device at 44.1 kHz. If no compatible device is
  available the frontend still runs, silently.

## Layout

- `src/libretro/`: minimal hand-written libretro ABI, core loader, host callbacks,
  video/audio/input handoff.
- `src/emu.rs`: emulation worker thread, commands, save states and memory cards.
- `src/sdl_input.rs`: SDL controller discovery, mapping, rumble, menu navigation
  and the shared SDL audio callback (including the splash motif).
- `src/splash.rs`: embedded logo and motif decoding, branding helpers.
- `src/app.rs`: splash, launcher, in-game overlay, settings and controller windows.
- `src/filebrowser.rs`: small built-in file picker.
- `assets/`: embedded logo, splash motif and desktop icon.
- `packaging/`: desktop entry.
