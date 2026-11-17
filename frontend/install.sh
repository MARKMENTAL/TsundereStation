#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
binary="$here/target/release/tsunderestation"
core="$here/../swanstation_libretro.so"

if [[ ! -x "$binary" ]]; then
    echo "error: $binary not found" >&2
    echo "build it first with: (cd \"$here\" && cargo build --release)" >&2
    exit 1
fi

install -Dm755 "$binary" "$prefix/bin/tsunderestation"
install -Dm644 "$here/assets/tsunderestation.png" \
    "$prefix/share/icons/hicolor/512x512/apps/tsunderestation.png"
install -Dm644 "$here/packaging/tsunderestation.desktop" \
    "$prefix/share/applications/tsunderestation.desktop"

if [[ -f "$core" ]]; then
    install -Dm644 "$core" "$prefix/lib/libretro/swanstation_libretro.so"
    echo "installed core: $prefix/lib/libretro/swanstation_libretro.so"
else
    echo "note: core not found at $core; install it separately or pass --core"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" >/dev/null 2>&1 || true
fi

echo "installed: $prefix/bin/tsunderestation"
echo "make sure $prefix/bin is in your PATH"
