#!/usr/bin/env bash
#
# Build TsundereStation AppImages for x86_64 and aarch64.
#
# The native x86_64 build uses the current linuxdeploy flow. The aarch64 build
# is cross-compiled with aarch64-linux-gnu-gcc and packaged with a manually
# assembled AppDir, because linuxdeploy does not support cross-deployment.
#
# Usage:
#   ./packaging/build-appimage.sh [--arch native|x86_64|aarch64|all]
#                                 [--skip-build] [--no-test]
#
# The resulting AppImages use the build host's glibc as their minimum. On
# Debian 13 / glibc 2.41, they target Debian 13 or newer. Build on an older
# distro if you need wider compatibility.
#
set -euo pipefail

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

for cmd in curl make cargo install readelf sha256sum tar python3; do
    command -v "$cmd" >/dev/null 2>&1 || die "missing required command: $cmd"
done

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
frontend="$(cd "$here/.." && pwd)"
repo="$(cd "$frontend/.." && pwd)"
dist="$frontend/dist"
tools="${TSUNDERESTATION_APPIMAGE_TOOLS:-$here/tools}"
desktop="$here/tsunderestation.desktop"
icon="$frontend/assets/tsunderestation.png"
collector="$here/collect-aarch64-libs.py"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$frontend/Cargo.toml" | head -n1)"
[[ -n "$version" ]] || die "could not read version from frontend/Cargo.toml"
release_tag="v${version}"
checksum_file="$dist/TsundereStation-${release_tag}-SHA256SUMS.txt"
release_file="$dist/TsundereStation-${release_tag}-release.md"
release_template="$here/release-template.md"
release_notes="$here/release-notes/v${version}.md"
[[ -f "$release_template" ]] || die "missing release template: $release_template"
[[ -f "$collector" ]] || die "missing aarch64 library collector: $collector"

requested_arch="native"
skip_build=0
run_test=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            [[ $# -ge 2 ]] || die "--arch requires x86_64, aarch64, native or all"
            requested_arch="$2"
            shift 2
            ;;
        --skip-build) skip_build=1; shift ;;
        --no-test) run_test=0; shift ;;
        -h|--help)
            cat <<'EOF'
Usage: ./packaging/build-appimage.sh [--arch native|x86_64|aarch64|all]
                                      [--skip-build] [--no-test]

Options:
  --arch ARCH   Build native/current architecture, x86_64, aarch64, or all.
                Default: native.
  --skip-build  Reuse existing core and frontend binaries.
  --no-test     Do not run the AppImage selftests after building.
  -h, --help    Show this help.

The aarch64 build requires:
  aarch64-linux-gnu-gcc, aarch64-linux-gnu-g++, aarch64-linux-gnu-strip,
  the aarch64 Rust std target, and arm64 SDL2 runtime libraries.

AppImage tools are cached in packaging/tools. Set
TSUNDERESTATION_APPIMAGE_TOOLS to use a different cache directory.

A successful build also writes a combined SHA256SUMS file and release draft
under dist/ for every architecture built in this invocation.
EOF
            exit 0
            ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

case "$requested_arch" in
    native|x86_64|aarch64|all) ;;
    *) die "--arch must be native, x86_64, aarch64 or all" ;;
esac

host_arch="$(uname -m)"
[[ "$host_arch" == "x86_64" ]] || die "this script currently builds x86_64/aarch64 AppImages from an x86_64 host"

case "$requested_arch" in
    native|x86_64) archs=(x86_64) ;;
    aarch64) archs=(aarch64) ;;
    all) archs=(x86_64 aarch64) ;;
esac

say "TsundereStation AppImage build"
say "  repo:    $repo"
say "  version: $version"
say "  arch:    ${archs[*]}"

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

tmp_dirs=()
cleanup() {
    for tmp in "${tmp_dirs[@]}"; do
        rm -rf "$tmp"
    done
}
trap cleanup EXIT

fetch_tool() {
    local url="$1" output="$2"
    mkdir -p "$tools"
    if [[ ! -s "$output" ]]; then
        say "Downloading $(basename "$output")..."
        curl -fL --retry 3 -o "$output.tmp" "$url"
        mv "$output.tmp" "$output"
    fi
    chmod +x "$output"
}

fetch_appimagetool() {
    fetch_tool \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
        "$tools/appimagetool-x86_64.AppImage"
}

fetch_linuxdeploy() {
    fetch_tool \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage" \
        "$tools/linuxdeploy-x86_64.AppImage"
}

check_aarch64_prerequisites() {
    for cmd in aarch64-linux-gnu-gcc aarch64-linux-gnu-g++ aarch64-linux-gnu-strip rustup; do
        command -v "$cmd" >/dev/null 2>&1 || die "missing aarch64 build command: $cmd"
    done
    [[ -f /usr/lib/aarch64-linux-gnu/libSDL2-2.0.so.0 ]] \
        || die "missing arm64 SDL2 runtime: /usr/lib/aarch64-linux-gnu/libSDL2-2.0.so.0"
    rustup target list --installed | grep -qx "aarch64-unknown-linux-gnu" \
        || die "missing Rust target aarch64-unknown-linux-gnu; run: rustup target add aarch64-unknown-linux-gnu"
    if [[ $run_test -eq 1 ]]; then
        command -v qemu-aarch64-static >/dev/null 2>&1 \
            || die "aarch64 selftest needs qemu-aarch64-static; install qemu-user-static or use --no-test"
    fi
}

bin_native="$frontend/target/release/tsunderestation"
core_native="$repo/swanstation_libretro.so"
cross_build="$dist/build/aarch64"
bin_cross="$frontend/target/aarch64-unknown-linux-gnu/release/tsunderestation"
core_cross="$cross_build/swanstation_libretro.so"

build_x86_64() {
    if [[ $skip_build -eq 0 ]]; then
        say "Building x86_64 core..."
        (cd "$repo" && make -f Makefile.libretro platform=unix -j"$(nproc)")
        say "Building x86_64 frontend..."
        (cd "$frontend" && cargo build --release)
    fi
    [[ -x "$bin_native" ]] || die "missing x86_64 frontend: $bin_native"
    [[ -f "$core_native" ]] || die "missing x86_64 core: $core_native"
    readelf -h "$bin_native" | grep -q "X86-64" || die "frontend is not x86_64"
    readelf -h "$core_native" | grep -q "X86-64" || die "core is not x86_64"
}

build_aarch64() {
    check_aarch64_prerequisites
    if [[ $skip_build -eq 0 ]]; then
        mkdir -p "$cross_build"
        say "Building aarch64 core..."
        local src_tmp
        src_tmp="$(mktemp -d)"
        tmp_dirs+=("$src_tmp")
        (cd "$repo" && tar \
            --exclude=.git \
            --exclude=frontend \
            --exclude='*.o' \
            --exclude='*.d' \
            --exclude='*.so' \
            -cf - Makefile.libretro link.T LICENSE src dep) \
            | (cd "$src_tmp" && tar -xf -)
        local git_version git_hash build_date
        git_version="$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo unknown)"
        git_hash="$git_version"
        build_date="$(date -u '+%Y-%m-%d %H:%M:%S')"
        (cd "$src_tmp" && make -f Makefile.libretro platform=unix \
            CPU_ARCH=aarch64 \
            CC=aarch64-linux-gnu-gcc \
            CXX=aarch64-linux-gnu-g++ \
            GIT_VERSION="$git_version" \
            GIT_HASH="$git_hash" \
            BUILD_DATE="$build_date" \
            -j"$(nproc)") 2>&1 | tee "$cross_build/core-build.log"
        grep -q "LD swanstation_libretro.so" "$cross_build/core-build.log" \
            || die "aarch64 core build did not link"
        install -Dm644 "$src_tmp/swanstation_libretro.so" "$core_cross"

        say "Building aarch64 frontend..."
        mkdir -p "$cross_build/link"
        ln -sfn /usr/lib/aarch64-linux-gnu/libSDL2-2.0.so.0 "$cross_build/link/libSDL2.so"
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
            CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
            CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
            PKG_CONFIG_ALLOW_CROSS=1 \
            RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-L$cross_build/link -C link-arg=-Wl,-rpath-link,/usr/lib/aarch64-linux-gnu" \
            cargo build --release --target aarch64-unknown-linux-gnu \
                --manifest-path "$frontend/Cargo.toml"
    fi
    [[ -x "$bin_cross" ]] || die "missing aarch64 frontend: $bin_cross"
    [[ -f "$core_cross" ]] || die "missing aarch64 core: $core_cross"
    readelf -h "$bin_cross" | grep -q "AArch64" || die "frontend is not aarch64"
    readelf -h "$core_cross" | grep -q "AArch64" || die "core is not aarch64"
}

add_docs_and_source() {
    local appdir="$1" core="$2"
    install -Dm644 "$core" "$appdir/usr/lib/libretro/swanstation_libretro.so"
    install -Dm644 "$repo/LICENSE" "$appdir/usr/share/doc/tsunderestation/LICENSE"
    local source_url
    source_url="$(git -C "$repo" remote get-url origin 2>/dev/null || true)"
    [[ -n "$source_url" ]] || source_url="https://github.com/libretro/swanstation"
    cat > "$appdir/usr/share/doc/tsunderestation/SOURCE" <<EOF
TsundereStation source code:
$source_url

This AppImage bundles the TsundereStation frontend and the SwanStation
libretro core, both distributed under the GNU General Public License v3.
See LICENSE in this directory.
EOF
}

packaged_artifact=""

package_x86_64() {
    local appdir="$dist/AppDir-x86_64"
    local out="$dist/TsundereStation-${version}-x86_64.AppImage"
    local stable="$dist/TsundereStation-x86_64.AppImage"

    rm -rf "$appdir"
    mkdir -p "$appdir"
    say "Deploying x86_64 frontend and libraries with linuxdeploy..."
    "$tools/linuxdeploy-x86_64.AppImage" --appimage-extract-and-run \
        --appdir "$appdir" \
        --executable "$bin_native" \
        --desktop-file "$desktop" \
        --icon-file "$icon"
    add_docs_and_source "$appdir" "$core_native"

    strip --strip-unneeded "$appdir/usr/bin/tsunderestation" 2>/dev/null || true
    strip --strip-unneeded "$appdir/usr/lib/libretro/swanstation_libretro.so" 2>/dev/null || true

    mkdir -p "$dist"
    rm -f "$out" "$stable"
    say "Creating x86_64 AppImage..."
    ARCH=x86_64 VERSION="$version" \
        "$tools/appimagetool-x86_64.AppImage" --appimage-extract-and-run --no-appstream \
        "$appdir" "$out"
    ln -sfn "$(basename "$out")" "$stable"
    packaged_artifact="$out"
}

package_aarch64() {
    local appdir="$dist/AppDir-aarch64"
    local out="$dist/TsundereStation-${version}-aarch64.AppImage"
    local stable="$dist/TsundereStation-aarch64.AppImage"

    rm -rf "$appdir"
    mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
        "$appdir/usr/share/icons/hicolor/512x512/apps"

    say "Assembling aarch64 AppDir..."
    install -m755 "$bin_cross" "$appdir/usr/bin/tsunderestation"
    install -m644 "$desktop" "$appdir/tsunderestation.desktop"
    install -m644 "$desktop" "$appdir/usr/share/applications/tsunderestation.desktop"
    install -m644 "$icon" "$appdir/tsunderestation.png"
    install -m644 "$icon" "$appdir/.DirIcon"
    install -m644 "$icon" "$appdir/usr/share/icons/hicolor/512x512/apps/tsunderestation.png"

    cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
set -eu
APPDIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export LD_LIBRARY_PATH="$APPDIR/usr/lib:$APPDIR/usr/lib/pulseaudio${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$APPDIR/usr/bin/tsunderestation" "$@"
EOF
    chmod 755 "$appdir/AppRun"

    say "Collecting aarch64 runtime libraries..."
    python3 "$collector" \
        --dest "$appdir/usr/lib" \
        --search /usr/lib/aarch64-linux-gnu \
        --search /lib/aarch64-linux-gnu \
        --pulseaudio-dir /usr/lib/aarch64-linux-gnu/pulseaudio \
        --root "$appdir/usr/bin/tsunderestation" \
        --root "$core_cross"

    # The host must provide the graphics/audio stack. Bundling Debian copies
    # of these breaks Mesa/EGL/Wayland on distros such as Asahi Linux.
    for pattern in \
        'libEGL.so*' 'libGL.so*' 'libGLX.so*' 'libOpenGL.so*' 'libGLdispatch.so*' \
        'libX11.so.6' 'libX11-xcb.so.1' 'libxcb.so.1' \
        'libwayland-client.so.0' 'libgbm.so.1' 'libdrm.so.2' \
        'libexpat.so.1' 'libasound.so.2'; do
        if compgen -G "$appdir/usr/lib/$pattern" >/dev/null; then
            die "aarch64 AppDir must not bundle host library: $pattern"
        fi
    done

    add_docs_and_source "$appdir" "$core_cross"

    aarch64-linux-gnu-strip --strip-unneeded \
        "$appdir/usr/bin/tsunderestation" \
        "$appdir/usr/lib/libretro/swanstation_libretro.so"

    mkdir -p "$dist"
    rm -f "$out" "$stable"
    say "Creating aarch64 AppImage..."
    ARCH=aarch64 VERSION="$version" \
        "$tools/appimagetool-x86_64.AppImage" --appimage-extract-and-run --no-appstream \
        "$appdir" "$out"
    ln -sfn "$(basename "$out")" "$stable"
    packaged_artifact="$out"
}

selftest_status="not run"
run_selftest() {
    local arch="$1" out="$2"
    selftest_status="not run"
    [[ $run_test -eq 1 ]] || return 0

    say "Running ${arch} AppImage selftest..."
    local tmp
    tmp="$(mktemp -d)"
    tmp_dirs+=("$tmp")
    local -a runner=()
    if [[ "$arch" == "aarch64" ]]; then
        runner=(qemu-aarch64-static)
    fi
    if ! XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" \
        "${runner[@]}" "$out" --appimage-extract-and-run --selftest 2>&1 \
        | tee "$tmp/selftest.log"; then
        die "${arch} AppImage selftest failed"
    fi
    grep -q "selftest ok" "$tmp/selftest.log" \
        || die "${arch} AppImage selftest did not report success"
    selftest_status="passed"
    say "${arch} AppImage selftest passed."
}

if [[ " ${archs[*]} " == *" x86_64 "* ]]; then
    fetch_linuxdeploy
fi
fetch_appimagetool

artifacts=()
artifact_arches=()
selftest_results=()

for arch in "${archs[@]}"; do
    packaged_artifact=""
    case "$arch" in
        x86_64)
            build_x86_64
            package_x86_64
            ;;
        aarch64)
            build_aarch64
            package_aarch64
            ;;
        *) die "internal error: unsupported arch $arch" ;;
    esac
    out="$packaged_artifact"
    [[ -n "$out" ]] || die "packaging did not produce an artifact for $arch"
    run_selftest "$arch" "$out"
    artifacts+=("$out")
    artifact_arches+=("$arch")
    selftest_results+=("$arch: $selftest_status")
done

downloads=""
checksums=""
expected=""
uploads=""
for i in "${!artifacts[@]}"; do
    artifact="${artifacts[$i]}"
    arch="${artifact_arches[$i]}"
    name="$(basename "$artifact")"
    sha="$(sha256sum "$artifact" | awk '{print $1}')"
    downloads+="- \`$name\` — Linux $arch"$'\n'
    checksums+="$sha  $name"$'\n'
    expected+="$name: OK"$'\n'
    uploads+="- Upload: $name"$'\n'
done

mkdir -p "$dist"
printf '%s' "$checksums" > "$checksum_file"

selftest_summary="${selftest_results[0]}"
for result in "${selftest_results[@]:1}"; do
    selftest_summary+="; $result"
done

build_date="$(date -u '+%Y-%m-%d %H:%M UTC')"
if [[ -f "$release_notes" ]]; then
    notes_file="$release_notes"
else
    notes_file=""
fi

awk -v tag="$release_tag" \
    -v downloads="$downloads" \
    -v checksums="$checksums" \
    -v checksum="$(basename "$checksum_file")" \
    -v expected="$expected" \
    -v uploads="$uploads" \
    -v selftest="$selftest_summary" \
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
    line = replace_all(line, "{{DOWNLOADS}}", downloads);
    line = replace_all(line, "{{CHECKSUMS}}", checksums);
    line = replace_all(line, "{{CHECKSUM}}", checksum);
    line = replace_all(line, "{{EXPECTED}}", expected);
    line = replace_all(line, "{{UPLOADS}}", uploads);
    line = replace_all(line, "{{SELFTEST}}", selftest);
    line = replace_all(line, "{{BUILD_DATE}}", build_date);
    line = replace_all(line, "{{HOST}}", host);
    line = replace_all(line, "{{GLIBC}}", glibc);
    line = replace_all(line, "{{GLIBC_REQUIREMENT}}", glibc_requirement);
    print line;
}' "$release_template" > "$release_file"

say ""
say "Built:"
for artifact in "${artifacts[@]}"; do
    say "  $artifact"
    say "    size: $(du -h "$artifact" | cut -f1)"
done
say "  checksum: $checksum_file"
say "  release:  $release_file"
say ""
say "Run an artifact with:"
for artifact in "${artifacts[@]}"; do
    say "  $artifact"
done
say ""
say "The AppImages require glibc >= the build host's glibc. If an AppImage is"
say "not executable directly, install FUSE2 or run it with:"
say "  <artifact> --appimage-extract-and-run"
