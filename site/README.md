# keyboard-it — landing / download page

Standalone static site with download links and install instructions.

## Deployment architecture

- **Primary: GitHub Pages** → https://kutayoz.github.io/keyboard-it/
  `.github/workflows/pages.yml` deploys automatically when `site/**` is pushed to `main`.
  Primary because `*.pages.dev` is SNI-filtered at ISP level in Turkey (RST on the TLS
  Client Hello); `github.io` is reachable.
- **Mirror: Cloudflare Pages** → https://keyboard-it.pages.dev
  Updated manually: `npx wrangler pages deploy site --project-name keyboard-it`
- **Installers: GitHub Releases.** The download buttons use
  version-independent `releases/latest/download/<fixed-name>` links. `install-macos.sh` pins
  both the image and checksum manifest to its `VERSION` to avoid a release-update race:
  - `keyboard-it-macos.dmg`
  - `keyboard-it-windows-x64.msi`
  The release job in `.github/workflows/build.yml` uploads these fixed-name copies on every
  `v*` tag, next to the versioned filenames (`keyboard-it-0.1.0.dmg` etc.) kept for archival.

## Contents

- `index.html` — single self-contained file (inline CSS plus a small vanilla JS scroll
  animation). Download links point to GitHub Releases; the site hosts no binaries.
- `keyboard-it.png` — favicon and hero icon.
- `install-macos.sh` — terminal installer (`curl … | sh`). The script downloads the DMG and SHA256SUMS from the same pinned GitHub Release, validates
  the image in a private temporary directory, mounts it, copies the `.app` to `/Applications`,
  and opens it. A missing/mismatched checksum stops installation before mounting. The app
  remains unsigned; a same-origin checksum does not establish publisher identity. The terminal
  install box in `index.html` points here. If the site domain changes, update `BASE_URL` at
  the top of the script (shown only in error messages) and the `curl …/install-macos.sh`
  address in `index.html`; the image and checksum links live on Releases.
- `downloads/` — empty in the repo (`.gitkeep`). CI outputs can be dropped here for local
  testing; binaries are never committed (`.gitignore`).

## Cutting a release

1. Bump the version in the root `Cargo.toml` under `[workspace.package]`, commit.
2. `git tag vX.Y.Z && git push origin vX.Y.Z` — CI builds, creates the Release, and adds the
   fixed-name copies. The site links pick up the new version automatically.
3. After the release succeeds, update `index.html`: the download version, required app
   version and version-pinned release/checksum links. Button links use fixed-name latest assets;
   file sizes are omitted to avoid stale metadata. Verify both downloads and SHA256SUMS exist.
4. Update `VERSION` in `install-macos.sh` to the same release. Verify the checksum failure
   path with mocked installer tools; never exercise installation on a real app as a test.
5. Push the site change (GitHub Pages deploys automatically); optionally refresh the
   Cloudflare mirror with wrangler.

## Local preview

```bash
python3 -m http.server 8099 --directory site
# http://localhost:8099
```
