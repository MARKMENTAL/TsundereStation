# Site downloads

The built AppImages are served from this directory by the static site, but the
binaries are not committed.

Before previewing the download links or deploying the site, copy the current
release artifacts here from `frontend/`:

```sh
cp dist/TsundereStation-0.3.3-x86_64.AppImage tsunderestation-site/downloads/
cp dist/TsundereStation-0.3.3-aarch64.AppImage tsunderestation-site/downloads/
```

Download artifacts are gitignored. Update the copy commands and `index.html`
when the version changes.

Verify the served files:

```sh
sha256sum TsundereStation-0.3.3-x86_64.AppImage
sha256sum TsundereStation-0.3.3-aarch64.AppImage
```

Both must match the SHA-256 values shown on the site.
