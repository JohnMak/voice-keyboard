#!/bin/bash
# Reset all macOS permissions for Voice Keyboard before reinstalling.
# Usage: ./scripts/reset-permissions.sh [--install]
#   --install  Also reinstall the app after reset

set -e

BUNDLE_ID="com.alexmak.voice-keyboard"
APP_NAME="Voice Keyboard"
APP_PATH="/Applications/${APP_NAME}.app"

echo "=== Resetting permissions for ${BUNDLE_ID} ==="

# 1. Kill the app if running
echo "[1/5] Stopping ${APP_NAME}..."
pkill -f "voice-keyboard-app" 2>/dev/null || true
pkill -f "voice-typer" 2>/dev/null || true
sleep 1

# 2. Reset TCC permissions (Microphone, Accessibility, Input Monitoring)
echo "[2/5] Resetting TCC permissions..."
tccutil reset Microphone "$BUNDLE_ID" 2>/dev/null && echo "  - Microphone: reset" || echo "  - Microphone: skipped"
tccutil reset Accessibility "$BUNDLE_ID" 2>/dev/null && echo "  - Accessibility: reset" || echo "  - Accessibility: skipped"
tccutil reset ListenEvent "$BUNDLE_ID" 2>/dev/null && echo "  - Input Monitoring: reset" || echo "  - Input Monitoring: skipped"
tccutil reset ScreenCapture "$BUNDLE_ID" 2>/dev/null && echo "  - Screen Recording: reset" || echo "  - Screen Recording: skipped"

# 3. Remove saved app state
echo "[3/5] Clearing app state..."
rm -rf ~/Library/Application\ Support/"${BUNDLE_ID}" 2>/dev/null || true
rm -rf ~/Library/Caches/"${BUNDLE_ID}" 2>/dev/null || true
rm -rf ~/Library/WebKit/"${BUNDLE_ID}" 2>/dev/null || true
rm -rf ~/Library/Saved\ Application\ State/"${BUNDLE_ID}.savedState" 2>/dev/null || true
defaults delete "$BUNDLE_ID" 2>/dev/null || true
echo "  Done"

# 4. Remove the app
echo "[4/5] Removing ${APP_PATH}..."
rm -rf "$APP_PATH"
echo "  Done"

# 5. Optionally reinstall
if [[ "$1" == "--install" ]]; then
    echo "[5/5] Installing new version..."
    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    REPO_DIR="$(dirname "$SCRIPT_DIR")"
    BUNDLE_SRC="${REPO_DIR}/src-tauri/target/release/bundle/macos/${APP_NAME}.app"
    VOICE_TYPER_SRC="${REPO_DIR}/target/release/voice-typer"

    if [[ ! -d "$BUNDLE_SRC" ]]; then
        echo "  ERROR: App bundle not found at ${BUNDLE_SRC}"
        echo "  Run 'cargo tauri build' first."
        exit 1
    fi

    cp -R "$BUNDLE_SRC" "$APP_PATH"
    if [[ -f "$VOICE_TYPER_SRC" ]]; then
        cp "$VOICE_TYPER_SRC" "$APP_PATH/Contents/MacOS/voice-typer"
        echo "  voice-typer binary copied"
    fi
    echo "  Installed to ${APP_PATH}"
else
    echo "[5/5] Skipping install (use --install to reinstall)"
fi

echo ""
echo "=== Done ==="
echo "Launch ${APP_NAME} — macOS will re-prompt for all permissions."
