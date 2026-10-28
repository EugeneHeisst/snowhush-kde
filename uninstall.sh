#!/bin/bash
qdbus6 org.snowhush.Kde /Snowhush org.snowhush.Kde.Quit >/dev/null 2>&1
kwriteconfig6 --file kwinrc --group Plugins --key snowhushEnabled --delete
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript snowhush >/dev/null
kpackagetool6 --type KWin/Script -r snowhush
rm -f "$HOME/.local/bin/snowhush-kde" "$HOME/.local/share/applications/snowhush-kde.desktop" "$HOME/.config/autostart/snowhush-kde.desktop"
qdbus6 org.kde.KWin /KWin reconfigure
