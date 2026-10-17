---
title: Pixel graphics
description: Show real pixels for YouTube video in the terminal with sixel or the kitty graphics protocol, the native GPU window on Windows Terminal, and full colour under tmux.
---

On a terminal with a graphics protocol, ytui shows real pixels instead of characters. That covers video, thumbnails,
avatars, banners and the 3D scenes. It asks the terminal what it supports when it starts.

![ytui drawing video with sixel graphics inside the terminal]({{ '/screenshots/sixel.jpg' | relative_url }})

## Which terminals work

| Protocol | Terminals |
|---|---|
| kitty graphics | kitty and Ghostty, also behind tmux |
| sixel | Windows Terminal, WezTerm, foot, xterm (`-ti vt340`), mlterm, Konsole and others |

Every other terminal gets the [character render modes](../render-modes/).

- **kitty:** images are compressed and drawn with Unicode placeholders, so they scroll and clip with
  the rest of the screen and work inside tmux. tmux needs `set -g allow-passthrough on`.
- **sixel:** each frame gets its own palette with a light dither.

With either protocol, ytui sends only the rows that changed since the last frame. When most of
the frame changed, it sends the whole frame instead. This matters most for slides, screen
recordings and talking heads. For example, lecture slides went from 27 KB to 1 KB per frame on
sixel. Set `YTUI_SIXEL_EXACT=1` to resend every row that changed even slightly.

## Turning it on and off

- `G` switches between pixels and characters. Your choice is saved.
- `--gfx auto|kitty|sixel|off` (or `YTUI_GFX`) overrides what ytui detects.
- On your own computer, pixel video is capped at 1920 px. Over SSH it's capped at 960 px and 12 fps, which keeps it to about
  4 MB/s. Change the caps with `--gfx-px N` and `--gfx-fps N` (0 means no limit).
- `--sixel-colors N` sets the colours per sixel frame, from 2 to 255. 64 is about 40% faster to send but looks blotchy on soft shading.

## Native video window (Windows Terminal)

![ytui in Windows Terminal with video in the native GPU window]({{ '/screenshots/native-window.jpg' | relative_url }})

On Windows Terminal with libmpv, video can play in a borderless window that mpv draws straight to the
GPU. That gives hardware-decoded video up to 1080p60. ytui keeps the window exactly over the
player area, inline, as the miniplayer or fullscreen. The window never takes focus, clicks pass through to ytui,
and it hides when you switch tabs or open a dialog.

`N` toggles it. `--no-native` turns it off for the session.

## mpv window (Linux and macOS)

On Linux and macOS, `N` plays the video in mpv's own window instead: hardware-decoded, up to 1080p60, in any
terminal, including ones without pixel graphics. ytui can't place a window over the terminal there, so it's an
ordinary window you can move or put full screen. ytui stays in charge: play, pause, seek, volume, captions and
the queue all work from the terminal.

In the window, `f` or a double-click goes full screen, `Esc` leaves it, `Space` pauses and the arrow keys seek.
Closing the window brings the video back into the terminal. ytui remembers your choice. It needs libmpv and
a desktop session (not SSH).

## Colours under tmux

ytui draws in 24-bit colour. If tmux doesn't know the outer terminal supports it, tmux rounds
every colour to 256, which bands gradients and shifts colours. ytui notices and dithers to the 256-colour
palette itself, and the status bar tells you how to fix it. Add this to `~/.tmux.conf` and reattach:

```
set -as terminal-features ',xterm*:RGB'
```

`--colors auto|256|true` (or `YTUI_COLORS`) overrides the detection.
