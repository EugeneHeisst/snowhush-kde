#!/bin/bash
# Builds Snowhush for KDE, installs it to ~/.local/bin with its KWin script, and starts it at login.
set -e
cd "$(dirname "$0")"
command -v cargo >/dev/null || source "$HOME/.cargo/env"
cargo build --release
install -Dm755 target/release/snowhush-kde "$HOME/.local/bin/snowhush-kde"
install -Dm644 snowhush-kde.desktop "$HOME/.local/share/applications/snowhush-kde.desktop"
install -Dm644 snowhush-kde.desktop "$HOME/.config/autostart/snowhush-kde.desktop"
kpackagetool6 --type KWin/Script -u kwin-script 2>/dev/null || kpackagetool6 --type KWin/Script -i kwin-script
kwriteconfig6 --file kwinrc --group Plugins --key snowhushEnabled true
qdbus6 org.kde.KWin /KWin reconfigure
# KWin caches compiled QML in memory: a changed main.qml takes effect only after logging out and back in.
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript snowhush >/dev/null
qdbus6 org.kde.KWin /KWin reconfigure
qdbus6 org.snowhush.Kde /Snowhush org.snowhush.Kde.Quit >/dev/null 2>&1 && sleep 0.5 || true
setsid -f "$HOME/.local/bin/snowhush-kde" >/dev/null 2>&1
echo "Snowhush is running: look for the snowflake in the system tray."
