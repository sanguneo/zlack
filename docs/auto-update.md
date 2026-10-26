# Automatic updates

Zlack uses the Tauri v2 updater plugin and publishes signed update artifacts through
GitHub Releases.

## One-time GitHub setup

Copy the values from the ignored local file `.tauri-private/updater.env` into
these GitHub Actions repository secrets:

- `TAURI_PRIVATE_KEY`
- `TAURI_KEY_PASSWORD`

The Release workflow passes them to the Tauri v2 bundler as
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

Keep an encrypted backup of `.tauri-private/updater.env`. Losing the private
key means existing installations cannot verify updates signed by a replacement
key.

## Publishing an update

1. Increase the version in `package.json`.
2. Run `npm run update-version`.
3. Push a matching `v*` tag.
4. Publish the draft GitHub Release created by the Release workflow.

The workflow uploads `latest.json`, signed updater bundles, and their
signatures. `createUpdaterArtifacts` is `"v1Compatible"`, so the bundles keep
the Tauri v1 formats (`.msi.zip`, `.AppImage.tar.gz`, `.app.tar.gz`) that
1.5.x installs expect. Installed applications check `latest.json` at startup and show
an install prompt (see `check_for_update` in `src-tauri/src/main.rs`) when a
newer version is available.

The updater public key lives in `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json` and must match the signing key above.
