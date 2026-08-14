# Automatic updates

Zlack uses the Tauri v1 updater and publishes signed update artifacts through
GitHub Releases.

## One-time GitHub setup

Copy the values from the ignored local file `.tauri-private/updater.env` into
these GitHub Actions repository secrets:

- `TAURI_PRIVATE_KEY`
- `TAURI_KEY_PASSWORD`

Keep an encrypted backup of `.tauri-private/updater.env`. Losing the private
key means existing installations cannot verify updates signed by a replacement
key.

## Publishing an update

1. Increase the version in `package.json`.
2. Run `npm run update-version`.
3. Push a matching `v*` tag.
4. Publish the draft GitHub Release created by the Release workflow.

The workflow uploads `latest.json`, signed updater bundles, and their
signatures. Installed applications check `latest.json` at startup and show
Tauri's built-in update dialog when a newer version is available.
