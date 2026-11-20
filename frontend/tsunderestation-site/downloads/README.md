# Site downloads

The built AppImage is served from this directory by the static site, but the
binary is not committed.

Before previewing the download link or deploying the site, copy the current
release artifact here from `frontend/`:

```sh
cp dist/TsundereStation-0.2.1-x86_64.AppImage tsunderestation-site/downloads/
```

The exact file is listed in `../../.gitignore`. Update that path and
`index.html` when the version changes.

Verify the served file:

```sh
sha256sum TsundereStation-0.2.1-x86_64.AppImage
```

It must match the SHA-256 shown on the site.
