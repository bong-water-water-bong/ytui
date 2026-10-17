---
title: Render modes
description: Play YouTube as ASCII art, glow, or Unicode mosaics in the terminal, and tune the picture with the look panel and auto-exposure.
---

Press `m` to cycle the character render modes, or start with `--mode`. Your choice is saved.

![Big Buck Bunny drawn as coloured ASCII characters in glow mode]({{ '/screenshots/glow.jpg' | relative_url }})

| Mode | What it looks like |
|---|---|
| `glow` | Shape-matched ASCII in boosted colour over a dimmed copy of the picture. Tune it in the look panel. |
| `art` | Clean ASCII art, and the default. It puts stroke characters (`_ - \| / \`) along edges, uses a short density ramp for tone, and puts symbols where there's fine detail, all in coloured ink on black. The characters hold steady between frames instead of flickering. |
| `ascii` | Two-tone shape-matched ASCII. Flat areas are clean colour, with glyphs only where there's edge detail. |
| `classic` | Shape-matched ASCII: coloured ink on a dark ground. |
| `sextant` | A 2×3 two-colour mosaic per cell. The sharpest mode. Needs a font with sextant characters. |
| `quadrant` | A 2×2 two-colour mosaic. |
| `halfblock` | Top and bottom half blocks. |
| `braille` | A 2×4 dot mosaic. |

For real pixels instead of characters, see [pixel graphics](../pixel-graphics/).

## The look panel

Press `o` to open the look panel in the top right. It has:

- **Mode:** the render mode.
- **Brightness, saturation and background:** these change glow mode. Background is how bright the dimmed picture
  behind the characters is.
- **Contrast:** changes every ASCII mode.
- **Auto-exposure:** on or off.

Use the arrow keys or `hjkl` to choose a row and change it, or click and drag the sliders. Your
settings are kept between runs. Press `o` or `Esc` to close the panel.

![The look panel open over glow-mode video]({{ '/screenshots/glow-look-panel.jpg' | relative_url }})

## Auto-exposure

Auto-exposure is on by default; `E` toggles it. It lifts dark video so you can still read it. It
measures each frame's brightness from the 1st to the 99th percentile, then raises the highlights to white with one
gain for all channels, so colours keep their hue. It eases in over about 0.2 seconds and resets when you seek.
Frames whose highlights already reach white are left alone. The gain is capped at 3.5×, so
fades to black don't turn into noise. `AE` on the player bar shows when it's on.

## How the ASCII modes pick characters

The ASCII modes match each character's *shape*, not just its brightness. They follow the technique in Alex Harri's
["ASCII characters are not pixels"](https://alexharri.com/blog/ascii-rendering). Each character is
drawn from a real monospace font and described by how much ink falls in six staggered sampling
circles. Each cell of the picture is sampled the same way, with contrast enhancement, and the
character that matches most closely wins. Set `YTUI_FONT` to the path of a monospace `.ttf` to use
that font's character shapes.
