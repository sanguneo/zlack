#!/bin/bash
set -e

echo "🚧 Starting Zlack Build (Unix)..."

# linuxdeploy-plugin-gtk currently forces `GDK_BACKEND=x11` in the AppImage runtime hook,
# which can make the UI appear tiny on HiDPI Wayland compositors (e.g., Hyprland).
# Patch the plugin before building, and set a far-future mtime so `wget -N` during
# bundling won't overwrite it.
TAURI_CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/tauri"
GTK_PLUGIN_URL="https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
GTK_PLUGIN_PATH="$TAURI_CACHE_DIR/linuxdeploy-plugin-gtk.sh"
mkdir -p "$TAURI_CACHE_DIR"
if [[ ! -f "$GTK_PLUGIN_PATH" ]]; then
  wget -q -N "$GTK_PLUGIN_URL" -O "$GTK_PLUGIN_PATH" || true
fi
if [[ -f "$GTK_PLUGIN_PATH" ]]; then
  chmod +x "$GTK_PLUGIN_PATH" || true
  sed -i 's/^export GDK_BACKEND=x11.*/export GDK_BACKEND="${ZLACK_GDK_BACKEND:-wayland,x11}" # Prefer Wayland; set ZLACK_GDK_BACKEND=x11 to force X11/' "$GTK_PLUGIN_PATH" || true
  touch -d '2038-01-01 00:00:00' "$GTK_PLUGIN_PATH" 2>/dev/null || true
fi

# Clean AppImage staging dir to make repeated builds idempotent.
# Without this, linuxdeploy-plugin-gtk can fail with e.g.:
# `ln: failed to create symbolic link ...: File exists`
APPIMAGE_SCRIPT="src-tauri/target/release/bundle/appimage/build_appimage.sh"
APPIMAGE_DIR="$(dirname "$APPIMAGE_SCRIPT")"
rm -rf "$APPIMAGE_DIR/zlack.AppDir" || true

# `tauri build` occasionally fails during the AppImage step due to `wget -4`
# forcing IPv4 (e.g., IPv6-only networks) or flaky downloads. When that happens,
# we patch the generated AppImage script to not force IPv4 and retry just that step.
#
# On modern distros, AppImage bundling can also fail when linuxdeploy tries to strip
# shared libraries that contain `.relr.dyn` (RELR relocations). linuxdeploy supports
# disabling strip via `NO_STRIP=1`. We default to disabling strip unless explicitly
# opted in via `ZLACK_APPIMAGE_STRIP=1`.
ZLACK_APPIMAGE_STRIP=${ZLACK_APPIMAGE_STRIP-0}
TAURI_BUILD_ENV=()
if [[ "$ZLACK_APPIMAGE_STRIP" != "1" ]]; then
  TAURI_BUILD_ENV+=(NO_STRIP=1)
fi
set +e
env "${TAURI_BUILD_ENV[@]}" npm run tauri build
TAURI_EXIT=$?
set -e

APPIMAGE_DEB_DIR="src-tauri/target/release/bundle/appimage_deb/data/usr"
APPIMAGE_OUT_GLOB="src-tauri/target/release/bundle/appimage/*.AppImage"

if [[ "$TAURI_EXIT" -ne 0 ]]; then
  if [[ -f "$APPIMAGE_SCRIPT" ]]; then
    echo "⚠️  Tauri build failed; retrying AppImage step with bundling patches..."
    # Tauri's generated script uses `wget -4` which can fail on IPv6-only networks.
    # Replace `wget -q -4 -N` with `wget -q -N` (keep caching behavior).
    sed -i 's/wget -q -4 -N /wget -q -N /g' "$APPIMAGE_SCRIPT"

    rm -rf "$APPIMAGE_DIR/zlack.AppDir" || true
    if [[ "$ZLACK_APPIMAGE_STRIP" != "1" ]]; then
      ( cd "$APPIMAGE_DIR" && NO_STRIP=1 bash "./$(basename "$APPIMAGE_SCRIPT")" )
    else
      ( cd "$APPIMAGE_DIR" && bash "./$(basename "$APPIMAGE_SCRIPT")" )
    fi

    if ! compgen -G "$APPIMAGE_OUT_GLOB" >/dev/null; then
      echo "❌ AppImage retry did not produce an AppImage."
      exit "$TAURI_EXIT"
    fi
  else
    exit "$TAURI_EXIT"
  fi
fi

DIST_DIR="dists"
mkdir -p "$DIST_DIR"

echo "📦 Copying artifacts to $DIST_DIR..."

# macOS DMG
DMG_PATH="src-tauri/target/release/bundle/dmg/*.dmg"
if compgen -G "$DMG_PATH" >/dev/null; then
  cp $DMG_PATH "$DIST_DIR/"
  echo "  ✅ DMG copied."
fi

# macOS App
APP_PATH="src-tauri/target/release/bundle/macos/*.app"
if compgen -G "$APP_PATH" >/dev/null; then
  cp -r $APP_PATH "$DIST_DIR/"
  echo "  ✅ .app copied."
fi

# Linux Deb
DEB_PATH="src-tauri/target/release/bundle/deb/*.deb"
if compgen -G "$DEB_PATH" >/dev/null; then
  cp $DEB_PATH "$DIST_DIR/"
  echo "  ✅ DEB copied."
fi

# Linux AppImage
APPIMAGE_PATH="src-tauri/target/release/bundle/appimage/*.AppImage"
if compgen -G "$APPIMAGE_PATH" >/dev/null; then
  cp $APPIMAGE_PATH "$DIST_DIR/"
  echo "  ✅ AppImage copied."
fi

echo "✨ Build complete! Artifacts are in the '$DIST_DIR' folder."
