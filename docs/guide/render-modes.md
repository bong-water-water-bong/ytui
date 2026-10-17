---
title: Video in the terminal
description: How ytui draws YouTube in the terminal as truecolour half blocks, why playback stays smooth, and what auto-exposure does.
---

ytui draws video as truecolour half blocks. Each terminal cell is the character `▀`: its
foreground colour is the top pixel and its background colour is the bottom pixel, so every cell
shows two pixels, one above the other. ffmpeg decodes each frame at exactly the size of the cell
grid, `cols × 2*rows` pixels, so nothing is scaled again in the terminal.

Thumbnails, avatars and banners are drawn the same way.

![Big Buck Bunny sitting in a meadow, drawn as half blocks, fullscreen and paused, with the player controls and the player bar at the bottom]({{ '/screenshots/halfblock.jpg' | relative_url }})

## Why it stays smooth

The frame rate is limited by how many bytes the terminal can parse, not by drawing. So ytui sends
as little as it can:

- **Only cells that changed are sent.** A cell keeps its colour until the picture there moves past
  a small tolerance.
- **Each frame fits a byte budget** based on the terminal's measured speed. After each big frame,
  ytui asks the terminal for the cursor position. The terminal can only answer once it has
  processed the frame. Until it answers, a newer frame replaces the waiting one instead of queueing
  behind it.

Still or slow pictures are sent exactly. Fast motion gets slightly coarser instead of dropping
frames. On a maximized GNOME Ptyxis (VTE) window, ytui holds 54–62 fps on 60 fps video.

## Auto-exposure

Auto-exposure is on by default; `E` toggles it. It lifts dark video so you can still read it. It
measures each frame's brightness from the 1st to the 99th percentile, then raises the highlights to white with one
gain for all channels, so colours keep their hue. It eases in over about 0.2 seconds and resets when you seek.
Frames whose highlights already reach white are left alone. The gain is capped at 3.5×, so
fades to black don't turn into noise. `AE` on the player bar shows when it's on.
