# Zlack

**Zlack** is a lightweight, optimized desktop wrapper for [Slack](https://slack.com/), built with [Tauri](https://tauri.app/). It provides a native application experience with robust desktop notifications, handling deep links and window focus correctly even when minimized.

![Zlack Icon](src-tauri/icons/128x128.png)

## 🚀 Features

*   **Native Desktop Notifications**: Integrated directly with Windows native toast notifications.
*   **Unread Tray & Title Badges**: The tray icon shows a red badge for unread DMs/@mentions and a blue badge for other unread messages, and the window title is prefixed with `!` for unread DMs — handy if you prefer ambient indicators over toast popups.
*   **Custom CSS & Icon**: Place `zlack.css`, `zlack.png`, `zlack.ico`, or `zlack-taskbar.png` next to the executable to customize Slack's UI and Zlack's running icons.
*   **Private WebView2 Runtime (optional)**: On Windows, Zlack uses a private `webview2-runtime` folder next to the executable when present, otherwise it falls back to the shared system WebView2 runtime.
*   **Smart Context**: Extracts `Team ID` and `Channel ID` from Slack's console logs to ensure notifications take you to the exact right place.
*   **Background Reliability**: Includes a custom rust backend to ensure clicking a notification properly restores the window from the system tray and focuses it.
*   **Multi-Workspace Support**: Handles navigation for multiple Slack workspaces via standard webview login.
*   **Lightweight**: Uses Tauri's minimal footprint (WebView2 on Windows) instead of a full Chromium bundle (Electron).

## 🛠 Tech Stack

*   **Frontend**: Vanilla HTML/JS (Slack Web Client) + `preload.js` for bridge.
*   **Backend**: Rust (Tauri) for system integration.
*   **Notification Engine**: `tauri-winrt-notification` for advanced Windows Toast features (Inputs, Activation Callbacks).

## 📦 Installation

Download the installer for your OS:

| Platform | File |
|----------|------|
| **Windows** | `Zlack_${version}_x64-setup.exe` (installer) or `Zlack_${version}_x64_en-US.msi` |
| **macOS** | `Zlack_${version}_x64.dmg` |

1.  Run the installer.
2.  Launch **Zlack**.
3.  Log in to your Slack workspaces.

## 🏗 Development

### Prerequisites

*   [Node.js](https://nodejs.org/)
*   [Rust & Cargo](https://rustup.rs/)
*   [Tauri CLI](https://tauri.app/v1/guides/getting-started/prerequisites)

### Commands

**Install Dependencies:**
```bash
npm install
```

**Run in Development Mode:**
```bash
npm run tauri dev
```
*Note: In `dev` mode, clicking notifications may not reliably restore the window due to Windows AUMID restrictions. This works fully in the built release.*

**Build for Production (Windows):**
```bash
npm run build:dist:windows
```
This will compile the application and place the installer (`.exe` and `.msi`) into the `dists/` folder.

**Build for Production (macOS/Linux):**
```bash
npm run build:dist:unix
```
This requires running on a Mac or Linux machine. It will generate `.dmg`/`.app` (macOS) or `.deb`/`.AppImage` (Linux) in the `dists/` folder.

**Optional private WebView2 runtime (Windows):**

By default Zlack uses the shared, system-wide WebView2 runtime. Because that runtime is shared, allowing it through a software firewall effectively allows *any* app to reach the internet through it.

To use a private runtime instead, get the **Fixed Version** WebView2 runtime for your architecture from the [WebView2 download page](https://developer.microsoft.com/microsoft-edge/webview2/), extract it, and place it next to `Zlack.exe` as:

```text
Zlack.exe
webview2-runtime/
  msedgewebview2.exe
  ...
```

If `webview2-runtime/msedgewebview2.exe` exists, Zlack uses it. Otherwise, it falls back to the shared system runtime.

## 🎨 Customization

Place optional customization files next to `Zlack.exe`:

```text
Zlack.exe
zlack.css          # injected into Slack
zlack.png          # preferred custom running icon
zlack.ico          # fallback custom running icon
zlack-taskbar.png  # optional Windows taskbar-only override
```

The custom icon is used for the window, taskbar, and tray badge base. If `zlack-taskbar.png` exists, it overrides only the Windows taskbar icon. The embedded exe/installer icon remains fixed until rebuilt.

## 🧩 How It Works

### Notification Interception
Slack's web client sends telemetry traces to `/traces/v1/list_of_spans`. Zlack's `preload.js` intercepts this network traffic:
1.  Captures `notification:sent` spans to reliably identify the `Team ID` and `Channel ID` associated with the event.
2.  Intercepts the browser's `Notification` API request.
3.  Merges the content with the captured network context and sends it to the Rust backend.

### Robust Window Restoration
Clicking a notification on Windows while an app is minimized is notoriously tricky due to OS foreground rules. Zlack solves this by:
1.  **Main Thread Architecture**: Creates notification objects directly on the main thread to ensure proper COM listener persistence.
2.  **Staged Restoration**: Explicitly calls `set_skip_taskbar(false)`, `unminimize()`, and `show()` in the correct order.
3.  **Focus Hack**: Uses a temporary "Always On Top" toggle to force the window into the foreground even if Windows tries to suppress it.

## 📄 License

MIT
