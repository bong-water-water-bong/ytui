---
title: Render modes
description: Play YouTube in the terminal as 24-bit colour braille or glow ASCII, and tune the picture with the look panel and auto-exposure.
---

ytui has two render modes, both in 24-bit colour. Press `m` to switch between them, or start with
`--mode`. Your choice is saved.

| Mode | What it looks like |
|---|---|
| `glow` | The default. Shape-matched ASCII in boosted colour over a dimmed copy of the picture. |
| `braille` | 2×4 braille dots in boosted colour over a dimmed copy of the picture. |

Thumbnails, avatars and banners are drawn as half blocks.

## Glow

Glow is translated from [react-video-ascii](https://github.com/LucasHJin/react-video-ascii) by
Lucas Jin (MIT). You can try the original in its [live demo](https://video-ascii-demo.vercel.app/).
ytui uses the demo's settings and picks the same characters it does. The licence and details are in
[NOTICE](https://github.com/bong-water-water-bong/ytui/blob/main/NOTICE).

For each cell of the picture:

- ytui samples the brightness in six circles.
- It raises each sample by a shape exponent, which sharpens edges. This is **Contrast** in the look panel, 2.0 by default.
- It picks the character from the demo's character set whose shape is nearest.
- It grades the colour in HSL, by **Brightness** and **Saturation** (both 1.25 by default).
- It draws the character in that colour over the same colour times **Background** (0.30 by default).

The character shapes are the table the demo computes for itself (DejaVu Sans Mono at its default
size), built into ytui, so the same picture gives the same characters on every system.

![Big Buck Bunny under an apple tree, drawn as coloured ASCII characters in glow mode, fullscreen]({{ '/screenshots/glow.jpg' | relative_url }})

## Braille

Braille is ytui's own extension of the glow look, on 2×4 braille dots. Each dot's brightness goes
through the same shape step, then an ordered-dither threshold. So brighter cells light more dots,
and the dots follow edges. Lit dots take the colour of their part of the cell. The ground takes the
rest, dimmed by Background.

![Big Buck Bunny leaping at a butterfly, drawn in braille mode, fullscreen, with the player bar at the bottom]({{ '/screenshots/braille.jpg' | relative_url }})

## The look panel

Press `o` to open the look panel in the top right. It has:

- **Mode:** braille or glow.
- **Brightness and saturation:** the HSL colour grade.
- **Background:** how bright the dimmed picture behind the characters or dots is.
- **Contrast:** the shape exponent. `[` and `]` change it without opening the panel.
- **Auto-exposure:** on or off.

Use the arrow keys or `hjkl` to choose a row and change it, or click and drag the sliders. Your
settings are kept between runs. Press `o` or `Esc` to close the panel.

![The look panel open over glow-mode video, with Mode, Brightness, Saturation, Background, Contrast and Auto-exposure]({{ '/screenshots/look-panel.jpg' | relative_url }})

## Auto-exposure

Auto-exposure is on by default; `E` toggles it. It lifts dark video so you can still read it. It
measures each frame's brightness from the 1st to the 99th percentile, then raises the highlights to white with one
gain for all channels, so colours keep their hue. It eases in over about 0.2 seconds and resets when you seek.
Frames whose highlights already reach white are left alone. The gain is capped at 3.5×, so
fades to black don't turn into noise. `AE` on the player bar shows when it's on.
