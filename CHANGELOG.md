# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Customization files**: `zlack.css` next to the executable is injected into Slack, `zlack.png` / `zlack.ico` can override the running window/taskbar/tray icon, and `zlack-taskbar.png` can override only the Windows taskbar icon.

## [1.2.0] - 2026-06-19

### Added
- **Unread Badges** ([#3](https://github.com/sanguneo/zlack/issues/3)): Zlack now mirrors Slack's unread state without relying on popup notifications — red for unread DMs / @mentions, blue for other unread messages. It is shown in three places: the system-tray icon swaps to a badged variant, the **Windows taskbar button gets an overlay badge** (`ITaskbarList3::SetOverlayIcon`, stamped with the unread DM/mention count when available), and the window title is prefixed with `!` when there are unread DMs / mentions. Detection is driven from the Slack tab title in `preload.js`; native rendering lives in `main.rs`.
- **Private WebView2 Runtime Support** ([#2](https://github.com/sanguneo/zlack/issues/2)): On Windows, Zlack now uses a private `webview2-runtime` folder next to the executable when present, otherwise it falls back to the shared system WebView2 runtime. This lets users scope a software-firewall rule to Zlack without requiring a separate fixed-runtime build.

### Changed
- **Notification context correlation**: `preload.js` now timestamps captured Slack telemetry and only attaches a team/channel to a notification when that context was captured close in time to it, so a notification without its own context no longer inherits the previously captured channel.
- **Tray icon startup**: The tray icon is normalised to the rendered 32px base at launch, so the larger bundled icon no longer briefly flashes before the first unread-state update.

### Fixed
- **Windows build script**: `npm run build:dist:windows` invoked a non-existent `tauri build:windows` subcommand and never actually built; it now runs `tauri build` and fails fast on a non-zero exit code.
- **Notification shim**: `Notification.removeEventListener` discarded its filtered result and never removed click handlers; it now reassigns the handler list.

### Removed
- **Starter scaffolding**: Removed the leftover Tauri template (`greet` demo `main.js`, `styles.css`, sample SVG assets) from `src/`; the bundled `index.html` is now a minimal branded splash placeholder.
- **Unused crate metadata**: Dropped the redundant `notification` Cargo feature (already covered by `notification-all`) and the unused `serde` and `serde_json` Windows dependencies. (The `windows` crate is retained — it now powers the taskbar overlay badge.)

## [1.1.2] - 2026-01-21

### Added
- **Automated Version Sync**: Implemented `scripts/update-version.js` to automatically sync `Cargo.toml` and `tauri.conf.json` versions with `package.json` before builds.

### Changed
- **Navigation Logic**: Refined notification handling to strictly navigate to the channel ONLY when the notification is clicked. Focusing the window independently no longer triggers channel navigation.
- **Build Process**: `npm run tauri` commands now execute the version synchronization script first via `pretauri`.

## [1.1.1] - 2026-01-12

### Added
- **User-Agent Injection**: Added Chrome 143+ User-Agent to fix "Browser Not Supported" errors on macOS/Windows.
- **Main Thread Architecture**: Implemented `run_on_main_thread` for notification toasts to ensure COM listener persistence.
- **Focus Hack**: Added "Always-On-Top" toggle strategy to force window foreground focus on Windows.

### Changed
- **Application Icon**: Updated to new brand logo across all formats (ICO, ICNS, PNG).
- **Close Behavior**: Clicking 'X' now hides the window to the System Tray instead of minimizing, improving restoration reliability.
- **Documentation**: Updated `README.md` with new architecture details and installation instructions.
- **Cleanup**: Removed `chrono` dependency and stripped verbose comments/logs from `main.rs` and `preload.js`.

### Fixed
- **Notification Clicks**: Resolved issue where clicking a notification failed to restore/focus the window from the tray.
- **Link Handling**: Fixed issue where internal Slack links (e.g., workspace sign-in) with `target="_blank"` unintendedly opened in the external system browser.
- **Rust Ownership**: Fixed `app_handle` borrow checker errors in the notification callback closure.

## [1.1.0] - 2026-01-09
- Initial Release
