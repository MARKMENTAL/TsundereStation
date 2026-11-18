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
- `frontend/packaging/` contains the desktop entry and AppImage build script.

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
save state. It does not need a display.

Build the AppImage:

```sh
./packaging/build-appimage.sh
./packaging/build-appimage.sh --skip-build   # reuse existing binaries
./packaging/build-appimage.sh --no-test      # skip the AppImage selftest
```

The AppImage is a local Debian 13 / glibc 2.41 build. There is no AppImage CI
yet. The build script runs the packaged binary's `--selftest` unless disabled.

## Version and artifact policy

- Current version: **0.1.1**.
- Do not bump the version for ordinary fixes or documentation changes unless
  explicitly asked.
- Do not rename the AppImage major/minor path casually. Rebuild the existing
  version in place for fixes.
- After rebuilding, report the new SHA256 of
  `dist/TsundereStation-0.1.1-x86_64.AppImage`.
- Documentation-only changes do not require rebuilding the AppImage.

## Source map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI flags, config load, core discovery, eframe window setup, `--selftest`. |
| `src/app.rs` | App state, splash, launcher, in-game pause overlay, settings/controller windows, toasts. |
| `src/theme.rs` | Theme catalog parsing, `UiTheme` derivation, contrast engine, theme picker rows, custom window headers. |
| `src/tv_ui.rs` | Large full-width TV rows and controller hints. |
| `src/filebrowser.rs` | Built-in file browser for games and BIOS images. |
| `src/config.rs` | Config schema, XDG paths, legacy migration, core search paths. |
| `src/emu.rs` | Emulation worker thread, commands, save states and memory cards. |
| `src/sdl_input.rs` | SDL controller discovery, mapping, rumble, navigation and shared audio callback. |
| `src/splash.rs` | Embedded logo/motif decoding and window icon. |
| `src/libretro/` | Hand-written libretro ABI, core loader, host callbacks, video/audio/input handoff. |
| `assets/themes.toml` | Embedded theme catalog. |
| `assets/tsunderestation.png` | Desktop icon and README logo. |
| `assets/tsunderestation-logo.jpg` | Splash/logo source image. |
| `assets/tsunderestation-splash.mp3` | Embedded splash motif. |

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
from SDL to libretro input state.

## UI and product decisions

- The TV UI is always enabled. Do not add a separate desktop-density mode
  without an explicit request.
- Typography is large: headings 32 px, body/buttons 20 px, small text 16 px,
  monospace 17 px.
- Launcher, file-browser and pause-menu rows are full-width and at least 52 px
  tall. Focused rows get a strong outline and controller hints stay visible.
- Controller navigation covers the launcher, file browser and pause overlay.
  Settings and Controller windows are mouse/keyboard for now.
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

`Config` uses serde defaults. New fields must be backward compatible with
existing config files. The app migrates the old `swanstation-ui` config/data
identity on first run.

Core discovery order includes: configured core path, `TSUNDERESTATION_CORE`,
`SWANSTATION_CORE`, paths next to the executable, the current directory,
`~/.local/lib/libretro`, and `/usr/lib/libretro`. `--core` overrides discovery.

Useful flags:

```text
--core <core.so>  --game <disc>  --boot-bios  --bios-dir <dir>
--no-splash  --splash  --selftest  --version
```

For isolated manual testing, set `XDG_CONFIG_HOME` and `XDG_DATA_HOME` to
temporary directories. `RUST_LOG=info` is useful for libretro/host diagnostics.

## Testing and verification

Minimum for code changes:

```sh
cargo fmt
cargo test
cargo clippy --all-targets
cargo build --release
./target/release/tsunderestation --selftest
```

Palette/contrast changes must keep these tests passing:

- `embedded_themes_meet_settings_contrast_floor`
- `filled_backgrounds_use_higher_contrast_text`
- `japs1me_pink_uses_black_text`
- `pathological_external_theme_is_repaired`

Visual checklist for UI changes:

- JAPS1ME Settings: black “Settings” header and black “All options” on pink.
- Focused launcher row, file-browser row and pause-menu row.
- Light and Solarized Light settings and warnings.
- Burgundy on Black repaired text.
- An external theme loaded from `~/.config/tsunderestation/themes.toml`.
- Settings/Controller/File Browser custom headers and close buttons.
- Toast text on info/warning/error backgrounds.

For AppImage changes, build the AppImage and run its selftest:

```sh
./packaging/build-appimage.sh --skip-build
sha256sum dist/TsundereStation-0.1.1-x86_64.AppImage
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
  the new hash.
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

- Software renderer only; hardware GL/Vulkan rendering, upscaling and PGXP are
  not wired up.
- One controller mapped to port 1; multitap is not exposed.
- Single-disc images and the first disc of an `.m3u` work; disc swapping is not
  exposed.
- Rewind, runahead and cheats are not exposed.
- Audio uses SDL's default device at 44.1 kHz and runs silently if no device is
  available.

Fix these only when the task explicitly asks for them.
