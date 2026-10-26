# TODO

## Verify the Windows taskbar overlay badge (#3)

**Status:** ships in 1.2.0, compiles + bundles, but **not yet visually verified** on the desktop.
The system-tray icon badge and the window-title `!` prefix are confirmed working; only the
taskbar **button overlay** (red/blue dot + unread count via `ITaskbarList3::SetOverlayIcon`)
still needs a real-screen check.

### How to verify
1. Fully quit any running Zlack (tray icon → **Quit Zlack**). Otherwise the single-instance
   plugin forwards to the old process and the new build never runs.
2. Run a **debug** build (debug builds write a diagnostic log; release builds do not):
   - `npm run tauri dev`, or
   - `src-tauri/target/debug/Zlack.exe`
3. Log into Slack and create unread state in *other* channels:
   - a DM or @mention → expect **red** + a number
   - a normal channel unread (no mention) → expect **blue**
   Keep the window **visible on the taskbar** (do not minimize to tray).
4. Wait ~10s, then read `%TEMP%\zlack-debug.log`
   (`C:\Users\<you>\AppData\Local\Temp\zlack-debug.log`) and look for:
   - `[update_badge] state=... count=... title=...` — detection + IPC fired
   - `[overlay] SetOverlayIcon(...) -> Ok/Err` — overlay actually applied

### Interpreting the log
- `SetOverlayIcon(...) -> Ok(())` **but nothing on the taskbar** → Windows setting, **not a bug**:
  Settings → Personalization → Taskbar → **"Show badges on taskbar buttons"** must be ON.
- `SetOverlayIcon(...) -> Err(...)` → COM / icon-creation issue; fix in
  `overlay_icon` (`src-tauri/src/icons.rs`).
- **No `[overlay]` lines at all** → state never became `mention`/`unread`; check title detection
  (`COUNT_MARKER` / `UNREAD_MARKER`) in `src-tauri/preload.js` against the current Slack tab-title
  format.

### Notes
- The overlay only shows when the window has a taskbar button (visible, not hidden to tray);
  the hidden-to-tray case is covered by the tray icon badge.
- Diagnostic logging is gated to debug builds (`#[cfg(debug_assertions)]` in `debug_log`);
  release builds emit nothing.
- Relevant code: `make_overlay_rgba`, `rgba_to_hicon`, `set_taskbar_overlay` in
  `src-tauri/src/main.rs`; detection in `src-tauri/preload.js` (`setupBadgeBridge`).

### Acceptance
- Red badge (with the DM/mention count) on the taskbar button when there are unread DMs/mentions.
- Blue badge when there are only other unread messages.
- Overlay cleared when everything is read.
- Tune color / size / number placement if it reads poorly at 16px.
