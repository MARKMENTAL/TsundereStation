#!/usr/bin/env bash
#
# Build a local AMD64 AppImage for TsundereStation.
#
# The resulting AppImage uses the build host's glibc as its minimum. On
# Debian 13 / glibc 2.41, it targets Debian 13 or newer. Build on an older
# distro if you need wider compatibility.
#
# Usage:
#   ./packaging/build-appimage.sh [--skip-build] [--no-test]
#
# Tools are cached in packaging/tools unless TSUNDERESTATION_APPIMAGE_TOOLS
# points elsewhere.
#
set -euo pipefail

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

for cmd in curl make cargo install strip readelf sha256sum; do
    command -v "$cmd" >/dev/null 2>&1 || die "missing required command: $cmd"
done

[[ "$(uname -m)" == "x86_64" ]] || die "this script builds AMD64/x86_64 AppImages only"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
frontend="$(cd "$here/.." && pwd)"
repo="$(cd "$frontend/.." && pwd)"
dist="$frontend/dist"
appdir="$dist/AppDir"
bin="$frontend/target/release/tsunderestation"
core="$repo/swanstation_libretro.so"
tools="${TSUNDERESTATION_APPIMAGE_TOOLS:-$here/tools}"
desktop="$here/tsunderestation.desktop"
icon="$frontend/assets/tsunderestation.png"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$frontend/Cargo.toml" | head -n1)"
[[ -n "$version" ]] || die "could not read version from frontend/Cargo.toml"
release_tag="v${version}"
checksum_file="$dist/TsundereStation-${release_tag}-SHA256SUMS.txt"
release_file="$dist/TsundereStation-${release_tag}-release.md"
release_template="$here/release-template.md"
release_notes="$here/release-notes/v${version}.md"
[[ -f "$release_template" ]] || die "missing release template: $release_template"

skip_build=0
run_test=1
for arg in "$@"; do
    case "$arg" in
        --skip-build) skip_build=1 ;;
        --no-test) run_test=0 ;;
        -h|--help)
            cat <<'EOF'
Usage: ./packaging/build-appimage.sh [--skip-build] [--no-test]

Build a local AMD64 AppImage for TsundereStation.

Options:
  --skip-build  Reuse existing core and frontend binaries.
  --no-test     Do not run the AppImage selftest after building.
  -h, --help    Show this help.

The AppImage tools are cached in packaging/tools. Set
TSUNDERESTATION_APPIMAGE_TOOLS to use a different cache directory.
The resulting AppImage requires the build host's glibc or newer.

A successful build also writes a SHA256SUMS file and release draft under dist/.
EOF
            exit 0
            ;;
        *)
            die "unknown argument: $arg (try --help)"
            ;;
    esac
done

say "TsundereStation AppImage build"
say "  repo:    $repo"
say "  version: $version"
host_name="unknown"
if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    host_name="${PRETTY_NAME:-unknown}"
fi
glibc_version="unknown"
if command -v ldd >/dev/null 2>&1; then
    glibc_version="$(ldd --version 2>/dev/null | head -n1 || true)"
fi
say "  host:    $host_name"
say "  glibc:   $glibc_version"
glibc_min="$(printf '%s\n' "$glibc_version" | grep -oE '[0-9]+\.[0-9]+' | head -n1 || true)"
if [[ -n "$glibc_min" ]]; then
    glibc_requirement="glibc ${glibc_min} or newer"
else
    glibc_requirement="glibc matching the build host or newer"
fi

if [[ "$skip_build" -eq 0 ]]; then
    say "Building core..."
    (cd "$repo" && make -f Makefile.libretro platform=unix -j"$(nproc)")
    say "Building frontend..."
    (cd "$frontend" && cargo build --release)
fi

[[ -x "$bin" ]] || die "missing frontend binary: $bin (run without --skip-build first)"
[[ -f "$core" ]] || die "missing core library: $core (run without --skip-build first)"
readelf -h "$bin" | grep -q "X86-64" || die "frontend binary is not x86_64"
readelf -h "$core" | grep -q "X86-64" || die "core library is not x86_64"
[[ -f "$desktop" ]] || die "missing desktop file: $desktop"
[[ -f "$icon" ]] || die "missing icon: $icon"

mkdir -p "$tools"
fetch_tool() {
    local url="$1" output="$2"
    if [[ ! -s "$output" ]]; then
        say "Downloading $(basename "$output")..."
        curl -fL --retry 3 -o "$output.tmp" "$url"
        mv "$output.tmp" "$output"
    fi
    chmod +x "$output"
}
fetch_tool \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage" \
    "$tools/linuxdeploy-x86_64.AppImage"
fetch_tool \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
    "$tools/appimagetool-x86_64.AppImage"

rm -rf "$appdir"
mkdir -p "$appdir"

say "Deploying frontend and libraries with linuxdeploy..."
"$tools/linuxdeploy-x86_64.AppImage" --appimage-extract-and-run \
    --appdir "$appdir" \
    --executable "$bin" \
    --desktop-file "$desktop" \
    --icon-file "$icon"

say "Adding bundled core, license and source note..."
install -Dm644 "$core" "$appdir/usr/lib/libretro/swanstation_libretro.so"
install -Dm644 "$repo/LICENSE" "$appdir/usr/share/doc/tsunderestation/LICENSE"
source_url="$(git -C "$repo" remote get-url origin 2>/dev/null || true)"
[[ -n "$source_url" ]] || source_url="https://github.com/libretro/swanstation"
cat > "$appdir/usr/share/doc/tsunderestation/SOURCE" <<EOF
TsundereStation source code:
$source_url

This AppImage bundles the TsundereStation frontend and the SwanStation
libretro core, both distributed under the GNU General Public License v3.
See LICENSE in this directory.
EOF

# linuxdeploy strips its own dependencies; strip the core and the application
# explicitly to keep the artifact smaller.
strip --strip-unneeded "$appdir/usr/bin/tsunderestation" 2>/dev/null || true
strip --strip-unneeded "$appdir/usr/lib/libretro/swanstation_libretro.so" 2>/dev/null || true

out="$dist/TsundereStation-${version}-x86_64.AppImage"
stable="$dist/TsundereStation-x86_64.AppImage"
mkdir -p "$dist"
rm -f "$out" "$stable"

say "Creating AppImage..."
ARCH=x86_64 VERSION="$version" \
    "$tools/appimagetool-x86_64.AppImage" --appimage-extract-and-run --no-appstream \
    "$appdir" "$out"

ln -sfn "$(basename "$out")" "$stable"

selftest_status="not run"
if [[ "$run_test" -eq 1 ]]; then
    say "Running AppImage selftest..."
    tmp="$(mktemp -d)"
    cleanup() {
        rm -rf "$tmp"
        rm -rf /tmp/tsunderestation-selftest
        rm -f /tmp/tsunderestation-selftest.ppm
        rm -f /tmp/tsunderestation-selftest.state
    }
    trap cleanup EXIT
    if ! XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" \
        "$out" --appimage-extract-and-run --selftest 2>&1 | tee "$tmp/selftest.log"; then
        die "AppImage selftest failed"
    fi
    grep -q "selftest ok" "$tmp/selftest.log" || die "AppImage selftest did not report success"
    selftest_status="passed"
    say "AppImage selftest passed."
fi

artifact_name="$(basename "$out")"
sha256="$(cd "$dist" && sha256sum "$artifact_name" | awk '{print $1}')"
build_date="$(date -u '+%Y-%m-%d %H:%M UTC')"
printf '%s  %s\n' "$sha256" "$artifact_name" > "$checksum_file"

if [[ -f "$release_notes" ]]; then
    notes_file="$release_notes"
else
    notes_file=""
fi

awk -v tag="$release_tag" \
    -v artifact="$artifact_name" \
    -v sha="$sha256" \
    -v checksum="$(basename "$checksum_file")" \
    -v selftest="$selftest_status" \
    -v build_date="$build_date" \
    -v host="$host_name" \
    -v glibc="$glibc_version" \
    -v glibc_requirement="$glibc_requirement" \
    -v notes_file="$notes_file" '
function replace_all(value, needle, replacement,    position) {
    while ((position = index(value, needle)) > 0)
        value = substr(value, 1, position - 1) replacement substr(value, position + length(needle));
    return value;
}
{
    if ($0 == "{{HIGHLIGHTS}}") {
        found = 0;
        if (notes_file != "") {
            while ((getline line < notes_file) > 0) {
                print line;
                found = 1;
            }
            close(notes_file);
        }
        if (!found)
            print "- Add release highlights here.";
        next;
    }
    line = $0;
    line = replace_all(line, "{{TAG}}", tag);
    line = replace_all(line, "{{ARTIFACT}}", artifact);
    line = replace_all(line, "{{SHA256}}", sha);
    line = replace_all(line, "{{CHECKSUM}}", checksum);
    line = replace_all(line, "{{SELFTEST}}", selftest);
    line = replace_all(line, "{{BUILD_DATE}}", build_date);
    line = replace_all(line, "{{HOST}}", host);
    line = replace_all(line, "{{GLIBC}}", glibc);
    line = replace_all(line, "{{GLIBC_REQUIREMENT}}", glibc_requirement);
    print line;
}' "$release_template" > "$release_file"

say ""
say "Built:"
say "  $out"
say "  $stable -> $(basename "$out")"
say "  size: $(du -h "$out" | cut -f1)"
say "  checksum: $checksum_file"
say "  release:  $release_file"
say ""
say "Run it with:"
say "  $stable"
say ""
say "The AppImage requires glibc >= the build host's glibc. If it is not"
say "executable directly, install FUSE2 or run:"
say "  $out --appimage-extract-and-run"
