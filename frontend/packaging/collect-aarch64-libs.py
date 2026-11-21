#!/usr/bin/env python3
"""Copy the runtime library closure of aarch64 ELF files into an AppDir.

linuxdeploy does not support cross-deploying from x86_64 to aarch64, so the
aarch64 AppDir is assembled manually. This helper walks DT_NEEDED recursively
and copies the matching arm64 libraries from the multiarch system paths.
"""

from __future__ import annotations

import argparse
import fnmatch
import os
import re
import shutil
import subprocess
import sys

NEEDED_RE = re.compile(r"\(NEEDED\).*\[([^\]]+)\]")

DEFAULT_EXCLUDES = (
    "ld-linux-aarch64.so.1",
    "libanl.so.1",
    "libasound.so.2",
    "libBrokenLocale.so.1",
    "libcidn.so.1",
    "libc.so.6",
    "libdl.so.2",
    "libdrm.so.2",
    "libexpat.so.1",
    "libgcc_s.so.1",
    "libgbm.so.1",
    "libm.so.6",
    "libnss_*.so*",
    "libOpenGL.so*",
    "libEGL.so*",
    "libGL.so*",
    "libGLX.so*",
    "libGLdispatch.so*",
    "libpthread.so.0",
    "libresolv.so.2",
    "librt.so.1",
    "libstdc++.so.6",
    "libthread_db.so.1",
    "libutil.so.1",
    "libwayland-client.so.0",
    "libX11.so.6",
    "libX11-xcb.so.1",
    "libxcb.so.1",
)


def read_needed(path: str) -> list[str]:
    try:
        result = subprocess.run(
            ["readelf", "-d", path],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return []
    needed = []
    for line in result.stdout.splitlines():
        match = NEEDED_RE.search(line)
        if match:
            needed.append(match.group(1))
    return needed


def is_excluded(name: str, patterns: tuple[str, ...]) -> bool:
    return any(fnmatch.fnmatch(name, pattern) for pattern in patterns)


def find_library(name: str, search_dirs: list[str]) -> str | None:
    for directory in search_dirs:
        candidate = os.path.join(directory, name)
        if os.path.isfile(candidate):
            return candidate
    return None


def copy_library(source: str, dest_dir: str, name: str) -> str:
    # libpulse loads its private common library from a pulseaudio
    # subdirectory next to libpulse.so.
    if name.startswith("libpulsecommon-"):
        dest_dir = os.path.join(dest_dir, "pulseaudio")
    os.makedirs(dest_dir, exist_ok=True)
    destination = os.path.join(dest_dir, name)
    if not os.path.exists(destination):
        shutil.copyfile(os.path.realpath(source), destination)
        os.chmod(destination, 0o755)
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dest", required=True, help="destination library directory")
    parser.add_argument(
        "--search",
        action="append",
        default=[],
        help="library search directory (repeatable)",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        help="extra soname exclude pattern (repeatable)",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=[],
        help="ELF file whose DT_NEEDED closure should be copied (repeatable)",
    )
    parser.add_argument(
        "--pulseaudio-dir",
        action="append",
        default=[],
        help="directory containing libpulsecommon-*.so (repeatable)",
    )
    args = parser.parse_args()

    search_dirs = args.search or [
        "/usr/lib/aarch64-linux-gnu",
        "/lib/aarch64-linux-gnu",
    ]
    search_dirs += args.pulseaudio_dir
    excludes = DEFAULT_EXCLUDES + tuple(args.exclude)

    queue: list[tuple[str, str | None]] = [(root, None) for root in args.root]
    seen_sonames: set[str] = set()
    seen_paths: set[str] = set()
    missing: list[str] = []

    while queue:
        path, soname = queue.pop(0)
        real = os.path.realpath(path)
        if soname is not None:
            if soname in seen_sonames:
                continue
            seen_sonames.add(soname)
            copy_library(real, args.dest, soname)
        elif real in seen_paths:
            continue

        seen_paths.add(real)
        for needed in read_needed(real):
            if is_excluded(needed, excludes) or needed in seen_sonames:
                continue
            found = find_library(needed, search_dirs)
            if found is None:
                missing.append(needed)
                continue
            queue.append((found, needed))

    if missing:
        print(
            "missing aarch64 libraries: " + ", ".join(sorted(set(missing))),
            file=sys.stderr,
        )
        return 1

    print(f"copied {len(seen_sonames)} aarch64 libraries to {args.dest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
