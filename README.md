# Snowhush for KDE Plasma

A port of [Snowhush](https://github.com/mbezruchko/Snowhush) (MIT, Mykhailo Bezruchko) to KDE Plasma 6 on Wayland.
Soft snow falls across the screen, settles on the tops of windows and on top of the bottom panel, slides off windows
you drag, drops in lumps when a window closes or minimizes, and parts under the cursor.

The snow simulation (`src/simulation.rs`, `src/layer.rs`, `src/geometry.rs`) is the upstream Windows port's, unchanged.
New here:

- `src/main.rs`: a click-through `wlr-layer-shell` surface per display (Top layer, or Bottom with *Behind Windows*),
  drawn with OpenGL ES 3 at the display's native resolution through `wp_viewporter`.
- `src/render.rs`: the upstream HLSL shaders ported to GLSL ES.
- `kwin-script/`: Wayland hides other windows and the pointer from clients, so a small KWin QML script sends
  window frames (front to back), desktop/minimize state and the cursor to `org.snowhush.Kde` over D-Bus.
- `src/tray.rs`: tray menu: Snowfall (Off, Light, Steady, Heavy, Variable), Accumulate, Melt Snow,
  Snow on Windows, Behind Windows, Quit. Settings live in `~/.config/snowhush-kde.conf`.

Snow hides while a full-screen window covers the display.

## Install

```sh
./install.sh     # builds, installs to ~/.local/bin, enables the KWin script, adds autostart, starts it
./uninstall.sh
```

Needs Rust (`rustup`), KWin 6 on Wayland and `kpackagetool6`/`qdbus6`.
