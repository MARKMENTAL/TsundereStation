# TsundereStation website

Static promotional site for TsundereStation. Lives in `tsunderestation-site/` and
deploys as plain static files.

## Stack

- **HTML** — single `index.html`, semantic sections, no templating.
- **CSS** — single `styles.css`, vanilla CSS only. No frameworks, preprocessors
  or build step.
- **Alpine.js 3** via CDN (`cdn.jsdelivr.net`, `defer`). Used only for the mobile
  menu, screenshot tabs, FAQ accordion and copy buttons.
- **Fonts** — system sans and monospace stacks. No webfonts.
- **JavaScript** — none beyond Alpine and `navigator.clipboard`.

## Files

- `index.html` — nav, hero, fact strip, screenshots, features, download,
  Getting Started, FAQ and footer.
- `styles.css` — design tokens, layout, components, responsive breakpoints and
  reduced-motion handling.
- `assets/` — self-contained site images:
  - `logo.png` from `../assets/tsunderestation.png`
  - `launcher.png`, `pause-menu.png`, `settings.png` from `../readme-screenshots/`
  - `evangelion-1.jpg`, `evangelion-2.jpg`, `wwf-1.jpg`, `wwf-2.jpg` from the
    same screenshot collection
- `downloads/TsundereStation-0.2.1-x86_64.AppImage` — current x86_64 AppImage,
  copied from `../dist/`. Download artifacts are gitignored; copy them here
  before previewing/deploying the download links.
- `downloads/TsundereStation-0.2.1-aarch64.AppImage` — current aarch64 AppImage,
  copied from `../dist/`. Also gitignored and copied manually.
- `downloads/README.md` — reminder for the download artifact workflow.

## Preview

From `frontend/`:

```sh
python3 -m http.server -d tsunderestation-site
```

Open the printed `http://localhost:8000` URL. Check a wide viewport and
`390×844` for the small-screen pass.

## Release updates

When a new AppImage is built:

1. Copy both artifacts into `downloads/`:

   ```sh
   cp dist/TsundereStation-<version>-x86_64.AppImage \
       tsunderestation-site/downloads/
   cp dist/TsundereStation-<version>-aarch64.AppImage \
       tsunderestation-site/downloads/
   ```

2. Update `index.html` in all relevant places:
   - hero `Download v<version>` label
   - download heading and filename
   - file size
   - SHA-256 and verify command
   - GitHub release link
   - footer version

3. Update the AppImage ignore path in `../.gitignore` if the filename changed.

4. Refresh `assets/` screenshots only if the UI changed.

5. Rebuild the AppImage’s generated `SHA256SUMS.txt`/release draft and compare
   the checksum shown on the site with the artifact actually being served.

The site intentionally does not use a build step or generated `release.json`.
Version and checksum values are static and must be updated as part of the
release checklist.

## Deployment

Deploy the entire `tsunderestation-site/` directory as static files. The
download artifacts are ignored by Git, so a deploy from a fresh checkout must
copy both `TsundereStation-<version>-x86_64.AppImage` and
`TsundereStation-<version>-aarch64.AppImage` into `downloads/` first.

No server-side code, analytics, cookies or external asset hosts are required
apart from the Alpine.js CDN. If the site must work fully offline, vendor Alpine
locally and update the script tag.

## Design notes

- Dark, PlayStation-inspired surfaces with a pink/periwinkle Tsundere accent.
- PS1 gradient used sparingly for dividers and selected accents.
- Aqua links/code and coral warnings keep the palette consistent with the app.
- Screenshots are real TsundereStation frames captured with `F12`; UI shots use
  the JAPS1ME theme.
- The hero screenshot chrome uses a Windows XP Media Center/Royale-inspired
  graphite-to-black glossy title bar with silver minimize/maximize controls and
  a red close button. Do not replace it with macOS traffic lights.
- Keep the page two files: structure in `index.html`, styling in `styles.css`.
