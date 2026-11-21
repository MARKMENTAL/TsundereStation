# AGENTS.md — TsundereStation frontend

This file is for AI coding agents and contributors working in `frontend/`.
Read it before changing the Rust UI. It records the decisions that are easy to
accidentally undo.

For user-facing build, usage, and install instructions, see
[`README.md`](README.md).

## What this is

TsundereStation is a standalone, Linux-native PlayStation 1 emulator front-end
for the SwanStation libretro core. It is written in Rust with egui/eframe and
SDL2, and loads `swanstation_libretro.so` directly. RetroArch is not required.

The front-end intentionally stays small:

- TV-sized, controller-friendly launcher, file browser and pause menu.
- Mouse/keyboard Settings and Controller windows.
- Save states, memory cards, BIOS management and core options.
- Memory Card Manager for `.srm`/`.mcd`/`.gme` cards, including DuckStation/RetroArch import and DexDrive conversion.
- Themeable UI with JAPS1ME as the default.
- A short branded startup splash and logo.

### Non-goals

Do not add these unless the product direction is explicitly changed:

- Discord integration or Rich Presence.
- RetroAchievements, trophies, or any achievement service.
- Telemetry, accounts, cloud saves, or online service integrations.
- Feature creep for its own sake.

The point of TsundereStation is a clean PS1 front-end with a good UI, not a
launcher platform.

## Repository boundaries

- `frontend/` is the Rust application. Most UI work belongs here.
- `../src/`, `../dep/` and `../Makefile.libretro` are the C++ SwanStation
  libretro core. Do not modify the core unless the task explicitly requires it.
- `../swanstation_libretro.so` is a built core artifact.
- `frontend/target/`, `frontend/dist/` and `frontend/packaging/tools/` are
  generated. Do not hand-edit them.
- `frontend/assets/` contains embedded assets: themes, logo, desktop icon and
  splash audio.
- `frontend/packaging/` contains the desktop entry, AppImage build script and
  aarch64 runtime-library collector.

## Build and test

From the repository root, build the core first:

```sh
make -f Makefile.libretro platform=unix -j"$(nproc)"
```

From `frontend/`, build and check the UI:

```sh
cargo build
cargo build --release
cargo test
cargo fmt
cargo clippy --all-targets
```

The default build uses the system SDL2 development package. To compile SDL2
from source:

```sh
cargo build --release --features bundled-sdl
```

Run the headless smoke test:

```sh
./target/release/tsunderestation --selftest
```

It boots BIOS-only content, runs frames, checks video/audio and round-trips a
save state. It does not need a display and forces software rendering.

To exercise the headless EGL/OpenGL path as well:

```sh
./target/release/tsunderestation --selftest-hw
```

This fails if the requested hardware renderer produces no hardware frames. On
machines without EGL/OpenGL it will report the expected fallback instead.

Build the AppImages:

```sh
./packaging/build-appimage.sh                    # native x86_64
./packaging/build-appimage.sh --arch aarch64     # cross-compiled aarch64
./packaging/build-appimage.sh --arch all         # both
./packaging/build-appimage.sh --arch all --skip-build   # reuse binaries
./packaging/build-appimage.sh --arch all --no-test      # skip selftests
```

The AppImages are local Debian 13 / glibc 2.41 builds. There is no AppImage CI
yet. The build script runs each packaged binary's `--selftest` unless disabled.

The aarch64 cross build needs `g++-aarch64-linux-gnu`,
`libc6-dev-arm64-cross`, arm64 SDL2 runtime libraries, the
`aarch64-unknown-linux-gnu` Rust target, and `qemu-aarch64-static` for the
selftest. linuxdeploy cannot cross-deploy, so the aarch64 AppDir is assembled
manually and packaged with `appimagetool`; keep
`packaging/collect-aarch64-libs.py` in sync with its runtime dependencies.

The aarch64 AppImage must leave the host graphics/audio stack alone. Do not
bundle `libEGL`, `libGL`, `libX11`, `libxcb`, `libwayland-client`, `libgbm`,
`libdrm`, `libexpat` or `libasound`; Debian copies of those break Mesa/EGL on
distros such as Asahi Linux. `build-appimage.sh` enforces this after collection.

After a successful build, the script also writes:

- `dist/TsundereStation-v<version>-SHA256SUMS.txt`
- `dist/TsundereStation-v<version>-release.md`

The draft uses `packaging/release-template.md` and injects
`packaging/release-notes/v<version>.md` under **Highlights**. If the notes file
is missing, it emits a placeholder. Do not hand-edit generated files in
`dist/`; update the template or per-version notes and rebuild.

## Version and artifact policy

- Current version: **0.3.0**.
- Do not bump the version for ordinary fixes or documentation changes unless
  explicitly asked.
- Do not rename the AppImage major/minor path casually. Rebuild the existing
  version in place for fixes.
- After rebuilding, report the new SHA256 of both
  `dist/TsundereStation-0.3.0-x86_64.AppImage` and
  `dist/TsundereStation-0.3.0-aarch64.AppImage`.
- Generated release files use tag `v<version>`, e.g.
  `TsundereStation-v0.3.0-SHA256SUMS.txt` and
  `TsundereStation-v0.3.0-release.md`. Upload both AppImages and the checksum
  file manually; paste the release draft as GitHub release notes.
- Documentation-only changes do not require rebuilding the AppImage.

## Source map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI flags, config load, core discovery, eframe window setup, `--selftest` and `--selftest-hw`. |
| `src/app.rs` | App state, splash, launcher, in-game pause overlay, settings/controller windows, toasts. |
| `src/theme.rs` | Theme catalog parsing, `UiTheme` derivation, contrast engine, theme picker rows, custom window headers. |
| `src/tv_ui.rs` | Large full-width TV rows and controller hints. |
| `src/filebrowser.rs` | Built-in file browser for games and BIOS images. |
| `src/config.rs` | Config schema, XDG paths, legacy migration, core search paths. |
| `src/memory_card/` | Vendored JAPS1ME card engine and Memory Card Manager UI. |
| `src/emu.rs` | Emulation worker thread, commands, save states, memory cards and disc swaps. |
| `src/sdl_input.rs` | SDL controller discovery, player slots, per-port mapping, rumble, navigation and shared audio callback. |
| `src/libretro/hardware.rs` | Headless EGL/OpenGL context, FBO readback, renderer fallback and worker startup preparation. |
| `src/splash.rs` | Embedded logo/motif decoding and window icon. |
| `src/screenshot.rs` | JPEG screenshot naming and encoding. |
| `src/libretro/` | Hand-written libretro ABI, core loader, host callbacks, video/audio/input handoff. |
| `assets/themes.toml` | Embedded theme catalog. |
| `assets/tsunderestation.png` | Desktop icon and README logo. |
| `assets/tsunderestation-logo.jpg` | Splash/logo source image. |
| `assets/tsunderestation-splash.mp3` | Embedded splash motif. |
| `packaging/build-appimage.sh` | Architecture-aware AppImage build and release metadata. |
| `packaging/collect-aarch64-libs.py` | Recursively copies the aarch64 runtime library closure into the cross-built AppDir. |
| `packaging/release-template.md` | Release-draft template. |
| `packaging/release-notes/` | Per-version release highlights. |
| `readme-screenshots/` | README-only UI/game screenshots; not embedded or packaged. |
| `tsunderestation-site/` | Static marketing site; plain HTML/CSS + Alpine CDN. |

## Architecture

Startup flows roughly like this:

1. `main.rs` parses flags and loads `Config`.
2. `Config` creates config/data directories and migrates the old
   `swanstation-ui` identity when needed.
3. `resolve_core()` finds `swanstation_libretro.so`.
4. `HostState` is installed as the libretro host singleton.
5. `App::new()` loads the core, loads the theme catalog, applies the selected
   theme, and starts the emulation worker.
6. `App` renders the splash, launcher, game view, pause overlay, settings,
   controller and file-browser windows.

Emulation runs on a worker thread. Video frames are handed to the egui texture
through `HostState`; audio goes through the shared SDL callback. Input is mapped
from SDL to per-port libretro snapshots: Player 1 writes port 1, Player 2 writes
port 2. Controller identity is persistent by GUID and bound per session by SDL
instance ID.

### Hardware rendering

The core can render with OpenGL through a headless EGL context. `src/libretro/hardware.rs`
provides a 1024×1024 RGBA8 FBO, fills the libretro `retro_hw_render_callback`,
and reads the FBO back into the existing `VideoMailbox` (row-flipped and made
opaque). The core always gets the 1× native resolution in this release.

The ordering is critical:

1. `main` resolves the core and starts `WorkerHandle` **before** `eframe`.
2. The worker calls `hardware::prepare_on_worker()` and creates the EGL context
   on its own thread.
3. The worker signals startup; only then does eframe create the UI's GL context.
4. `App::new` receives the already-started worker instead of spawning it.

Do not create the worker's EGL context after eframe is live: some Mesa/EGL/X11
combinations fail `eglMakeCurrent` in that situation. The context is kept alive
across core restarts and only dropped when the worker exits.

`swanstation_GPU_Renderer` accepts Auto/OpenGL/Software. Unsupported Vulkan/D3D
values are filtered out and old config values fall back to Auto. If EGL/OpenGL
setup fails, `SET_HW_RENDER` returns false and the core uses the software
renderer. Keep the AppImage free of bundled `libEGL`, `libGL`, X11/Wayland,
GBM/DRM and ALSA libraries; those must come from the host.

## UI and product decisions

- The TV UI is always enabled. Do not add a separate desktop-density mode
  without an explicit request.
- Typography is large: headings 32 px, body/buttons 20 px, small text 16 px,
  monospace 17 px.
- Launcher, file-browser and pause-menu rows are full-width and at least 52 px
  tall. Focused rows get a strong outline and controller hints stay visible.
- Controller navigation covers the launcher, file browser and pause overlay.
  Player 1 drives menus; Player 2 is game-only. If Player 1 is absent, Player 2
  can drive the UI as a fallback. Settings, Controller and Memory Card Manager
  windows are mouse/keyboard for now.
- Player 2 is opt-in. SwanStation port 2 stays disabled until a controller is
  assigned, and is set back to `RETRO_DEVICE_NONE` when cleared or disconnected.
- The press that dismisses the splash is consumed until release, matching the
  in-game menu-resume behavior.
- The Memory Card Manager opens from the launcher and uses the same custom
  accent header as Settings. Keep one outer scroll area for its details pane;
  nesting save-list and detail scroll areas makes lower controls unreliable.
- `F12` takes a JPEG screenshot of the game frame. The pause menu also exposes
  **Take screenshot (F12)**.
- The FPS/status overlay reports measured emulation frames per second from the
  worker's monotonic frame counter. Do not replace it with `HostState::fps()`,
  which is the core-reported frame-pacing target.
- Multi-disc `.m3u` content adds **Swap disc…** to the pause menu. The disc
  picker must stay controller-navigable and must call disc-control functions on
  the emulation worker thread, never directly from the UI thread.
- JAPS1ME is the default theme. Do not change the default without an explicit
  request.
- Keep service integrations and extra platforms out of scope (see Non-goals).
- Use semantic `UiTheme` colors in UI code. Do not introduce hardcoded theme
  colors (text, surfaces, accents, warnings) outside the contrast helpers. The
  game view's black backdrop and asset tints are intentional exceptions.

## Theme system

The embedded catalog is `assets/themes.toml`. It currently contains 39 themes in
display order. Optional external themes live at:

```text
~/.config/tsunderestation/themes.toml
```

External entries are appended to the embedded list. An entry with the same name
replaces the built-in theme in place. The Appearance screen has a **Reload
themes** button; parse/read errors are shown there and as a toast.

### Theme file format

```toml
["My Theme"]
foreground = "#f5f5f4"
background = "#101018"
normal = ["#101018", "#ff5555", "#55ff55", "#ffff55", "#5555ff", "#ff55ff", "#55ffff", "#f5f5f4"]
bright = ["#333340", "#ff7777", "#77ff77", "#ffff77", "#7777ff", "#ff77ff", "#77ffff", "#ffffff"]
```

- Each table is one theme.
- `normal` and `bright` must each contain exactly eight `#rrggbb` colors.
- Source order is preserved. The `toml` dependency must keep the
  `preserve_order` feature enabled.
- Do not change the file shape without migrating the embedded file, external
  parsing, the theme picker and the tests together.

### Color mapping

- Accent: `normal[5]`
- Warning: `normal[3]`
- Error: `normal[1]`
- Info: `normal[4]`
- `surface` and `surface2` are mixes of background toward foreground.
- `focus_fill` is a blend of background toward accent.

### Contrast contract

Enabled UI text must meet at least **2.5:1** contrast against its actual
background. The tests enforce this for every embedded theme plus a pathological
external theme.

Rules in `theme.rs`:

- `higher_contrast_text(background)` chooses black or white, whichever has the
  higher contrast ratio.
- `UiTheme.on_accent` uses `higher_contrast_text(accent)`.
- `UiTheme.on_color(color)` uses `higher_contrast_text(color)` for filled
  accent, focus, selected and toast backgrounds.
- `ensure_contrast(color, background)` keeps readable colors unchanged and moves
  low-contrast colors toward black or white by the smallest amount needed.
- `text` is repaired against `surface2`, the hardest text background. This also
  keeps it readable on `surface` and `background`.
- `text_on_hover` is repaired against `hover`.
- `muted` is repaired against `surface2` for weak labels and text-edit hints.
- `warning`, `error` and `info` are repaired against `surface`.
- The theme picker uses `Theme::preview_text_color()` for the theme name. ANSI
  swatches still show the exact source palette.
- Focused TV rows use `on_color(focus_fill)`. Do not use `on_accent` there:
  `focus_fill` is a different background and produces different results.

The `2.5:1` floor is intentional. Do not raise it to WCAG AA 4.5:1 without
checking every theme and the visual result.

### Widget text states

Do not set a global `visuals.override_text_color`; it also overrides hovered and
active widget text. `Theme::apply()` sets colors per state:

- Non-interactive labels/headings: `text`
- Inactive widgets: `text`
- Hovered widgets: `text_on_hover` on `hover`
- Active/focused widgets: `on_accent` on `accent`
- Selected labels: `selection.bg_fill = accent`,
  `selection.stroke = on_accent`
- Text cursor: `text`
- Weak labels and hints: `muted`

`RichText::strong()` uses `strong_text_color()`, which comes from the active
widget color. Any explicit strong label that is not on an accent background must
set its own color, as the settings labels do.

### Custom window headers

egui's native window title uses the ordinary text color, not `on_accent`. The
Settings, Controller and File Browser windows therefore use:

- `.title_bar(false)`
- `theme::window_frame(...)` with zero content margin
- `theme::window_header(...)` with an accent background, `on_accent` title and a
  custom close button
- `theme::window_body(...)` for the padded body

Do not re-enable the native title bar for these windows without solving the
light-accent/white-text contrast bug. Without a title bar, egui falls back to
drag-anywhere window movement; this is expected.

## egui gotchas

- `ui.selectable_label` uses `selection.stroke` for selected text. `selection`
  is also used for text-selection colors and the focused text-edit border.
- `Window::title_bar(false)` makes the window draggable from anywhere.
- Custom clickable rows in loops need stable unique IDs. `tv_row` takes a key;
  the theme picker uses `("theme-picker", &theme.name)`. Do not return to
  automatic IDs for repeated rows.
- `.strong()` labels inherit the active widget text color.
- Window content margin is normally applied through the window frame. The custom
  header windows deliberately set it to zero and pad the body themselves.
- Keep the embedded and external theme order stable; `toml` must preserve order.

## Config, paths and core discovery

Config follows the XDG base directory spec:

- Config: `~/.config/tsunderestation/config.toml`
- Optional themes: `~/.config/tsunderestation/themes.toml`
- BIOS: `~/.local/share/tsunderestation/bios/`
- Memory cards: `~/.local/share/tsunderestation/saves/*.srm`
- Save states: `~/.local/share/tsunderestation/states/*.state`
- Screenshots: `~/.local/share/tsunderestation/screenshots/*.jpg`

`Config` uses serde defaults. New fields must be backward compatible with
existing config files. `memory_card_source_dir` remembers the last external
DuckStation/RetroArch card directory. The app migrates the old `swanstation-ui`
config/data identity on first run.

Core discovery order includes: configured core path, `TSUNDERESTATION_CORE`,
`SWANSTATION_CORE`, paths next to the executable, the current directory,
`~/.local/lib/libretro`, and `/usr/lib/libretro`. `--core` overrides discovery.

Useful flags:

```text
--core <core.so>  --game <disc>  --boot-bios  --bios-dir <dir>
--no-splash  --splash  --selftest  --selftest-hw  --version
```

For isolated manual testing, set `XDG_CONFIG_HOME` and `XDG_DATA_HOME` to
temporary directories. `RUST_LOG=info` is useful for libretro/host diagnostics.

## Memory card manager

The Memory Card Manager is a vendored, UI-adapted port of the JAPS1ME card
engine. Keep the attribution headers in `src/memory_card/` and keep the
GPL-3.0-or-later license compatible.

- `src/memory_card/card.rs` — raw card parsing, slots, link chains and titles.
- `src/memory_card/gme.rs` — DexDrive `.gme` parsing, including headerless files.
- `src/memory_card/import.rs` — save import and overwrite.
- `src/memory_card/delete.rs` — save deletion.
- `src/memory_card/icon.rs` / `icon_png.rs` — save icon decode and PNG export.
- `src/memory_card/io.rs` — format detection, directory scanning, naming,
  backups and atomic writes.
- `src/memory_card/ui.rs` — egui window, card lists, save lists and confirmations.

Card invariants:

- Raw card = exactly 131072 bytes starting with `MC`.
- `.gme` = 3904-byte DexDrive header + 131072-byte raw card (134976 bytes).
- `.srm`, `.mcd` and `.mcr` are the same raw card layout; only naming
  conventions differ.
- TsundereStation local cards live in `Config::save_dir` and must use `.srm`
  names matching the game filename.
- DuckStation uses `.mcd`; RetroArch uses `.srm`.
- When copying a DuckStation card into TsundereStation, strip a trailing
  `_SLOT` suffix and use `.srm` by default, but keep the destination name
  editable.
- A DexDrive `.gme` is read-only as a source; copying or importing converts it
  to raw bytes.

Safety rules:

- All writes are staged in a temporary file, re-parsed and then persisted.
- Never modify DuckStation/RetroArch source cards during import; copy into the
  local directory first.
- Back up targets to `<card>.japs1me-bak` before overwrite or delete.
- `.gme` conversion refuses to overwrite an existing output.
- Destructive operations require a modal confirmation.
- The TsundereStation UI intentionally has no install-back action. It imports
  and copies cards into TsundereStation only; moving a card back to another
  emulator is a manual file operation.
- Do not use a user's only card for tests. Use temp dirs and synthetic cards;
  treat `JAPS1ME/save-games/` as read-only fixtures.

`frontend/JAPS1ME/` is a separate nested Git repository and a reference
implementation, ignored by `frontend/.gitignore`. It is not a build dependency.
The vendored copy under `src/memory_card/` is the frontend source of truth.
Changes to JAPS1ME must be tested and committed in that repository. Its dev
branch adds a `TsundereStation` install target for the JAPS1ME CLI (`.srm`,
detected from a `tsunderestation` path component); that is separate from the
TsundereStation UI, which has no install-back action.

## Screenshots and disc control

Screenshots use the latest RGB frame from `VideoMailbox`, encode it as JPEG at
quality 92, and write to `config::screenshots_dir()`:

```text
~/.local/share/tsunderestation/screenshots/gamename-YYYYMMDDHHMMSS.jpg
```

Filenames use local time. If two screenshots have the same timestamp, append
`-1`, `-2`, and so on. The game title/stem is sanitized for filesystem use.

Disc swapping uses the libretro disk-control interface:

- `HostState` stores the callbacks registered by the core during `retro_init`.
- `RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION` returns 1.
- Both the ext and legacy interface calls are accepted.
- The emulation worker performs eject → select image → close. Never call these
  callbacks from the egui/UI thread.
- `WorkerEvent::DiscInfo` carries labels and the current index back to the UI.
- Swap is available only when more than one image exists, normally an `.m3u`
  playlist. Do not add a swap row for single-disc content.
- Clear stored callbacks when the core shuts down.

## Marketing site

`tsunderestation-site/` is a self-contained static promo site with no build step.

- `index.html` is the page; `styles.css` is all styling.
- Alpine.js 3 comes from the CDN and is used only for mobile nav, screenshot
  tabs, FAQ and copy buttons.
- Site images live in `tsunderestation-site/assets/` and are copied from
  `assets/`/`readme-screenshots/`; keep the site deployable on its own.
- The current AppImage is copied into `tsunderestation-site/downloads/` but is
  ignored by `frontend/.gitignore`. Copy it again before a fresh deploy.
- Version, filename, size, SHA-256 and GitHub release links in `index.html` are
  static and must be updated with each release.
- Preview with `python3 -m http.server -d tsunderestation-site` from `frontend/`.
- No Rust code, AppImage rebuild or release-draft changes are required for site
  edits unless assets/version metadata change.

## Testing and verification

Minimum for code changes:

```sh
cargo fmt
cargo test
cargo clippy --all-targets
cargo build --release
./target/release/tsunderestation --selftest
./target/release/tsunderestation --selftest-hw
```

Palette/contrast changes must keep these tests passing:

- `embedded_themes_meet_settings_contrast_floor`
- `filled_backgrounds_use_higher_contrast_text`
- `japs1me_pink_uses_black_text`
- `pathological_external_theme_is_repaired`

Screenshot and disc-control changes must keep these tests passing:

- screenshot filename sanitization and collision suffix
- JPEG output validation
- disc-control mock eject/select/close ordering and failure recovery

Memory-card changes must keep the ported card tests passing, including:

- `.gme` canonical/headerless parsing
- raw card parsing and link chains
- import, overwrite, duplicate and full-card rejection
- delete and re-parse
- convert refusing overwrite
- DuckStation `_SLOT` stripping and TsundereStation `.srm` naming
- backup creation and atomic copy

When modifying `frontend/JAPS1ME/`, also run its own checks:

```sh
cd JAPS1ME
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Visual checklist for UI changes:

- JAPS1ME Settings: black “Settings” header and black “All options” on pink.
- Focused launcher row, file-browser row and pause-menu row.
- Light and Solarized Light settings and warnings.
- Burgundy on Black repaired text.
- An external theme loaded from `~/.config/tsunderestation/themes.toml`.
- Settings/Controller/File Browser custom headers and close buttons.
- Player 1 and Player 2 controller rows and their Controller-window assignments.
- Two controllers in a two-player game, including Player 2 hotplug and clearing
  Player 2 back to **None**.
- Holding Cross or Enter through the splash does not activate a launcher row.
- GPU Renderer Auto selects OpenGL and the FPS overlay shows `HW`; selecting
  Software shows `SW` after a core restart.
- Disabling EGL/OpenGL causes a clean software fallback with a warning rather
  than a crash.
- Toast text on info/warning/error backgrounds.
- Memory Card Manager: launcher entry, local card list, save list and icon preview.
- `.gme` warning and conversion, DuckStation card copy, duplicate import confirmation, delete confirmation and icon export.
- F12 and pause-menu screenshot actions write the expected JPG path/name.
- A multi-disc `.m3u` shows the disc picker; a single-disc image shows no swap row.

For AppImage changes, build both architectures and run their selftests:

```sh
./packaging/build-appimage.sh --arch all --skip-build
sha256sum dist/TsundereStation-0.3.0-x86_64.AppImage
sha256sum dist/TsundereStation-0.3.0-aarch64.AppImage
```

## Change checklist

Before finishing:

- Keep the change focused; don't refactor unrelated code.
- Keep the C++ core untouched unless required.
- Keep JAPS1ME as the default and preserve external-theme behavior.
- Keep the 2.5:1 contrast contract and its tests.
- Use `UiTheme` colors instead of hardcoded UI colors.
- Run fmt, tests, clippy, release build and selftest.
- Rebuild the AppImage in place only when code/assets are affected, and report
  the new hash plus the generated release/checksum paths.
- Update `README.md` or this file when behavior, commands or decisions change.

## Branding

The mascot is **The Tsundere**. The README tagline has her call someone a baka
for using a non-free PS1 emulator like DuckStation. Keep the tone
tongue-in-cheek.

DuckStation is source-available under CC BY-NC-ND 4.0, which is not a
free-software license. The joke is about licensing, not price or closed source;
do not describe DuckStation as paid or closed source. This project is not
affiliated with or endorsed by any anime or game rights holders.

## Known limitations

- Hardware rendering is OpenGL-only and fixed at 1× native resolution.
  Vulkan/D3D, upscaling and PGXP are not wired up. Software rendering remains
  available as a fallback.
- Two controllers mapped to ports 1 and 2; multitap is not exposed.
- Multi-disc swapping requires an `.m3u` playlist; single-file images expose one
  image only.
- Rewind, runahead and cheats are not exposed.
- Audio uses SDL's default device at 44.1 kHz and runs silently if no device is
  available.

Fix these only when the task explicitly asks for them.
