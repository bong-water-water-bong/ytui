# ytui

YouTube in the terminal: search, browse, and watch videos rendered as truecolour ASCII / Unicode mosaics.

## Requirements

- `yt-dlp` (plus Node.js 22+ or Deno, which yt-dlp needs for YouTube)
- `ffmpeg`
- a truecolour terminal; a font with Unicode sextants for `sextant` mode

## Build

```
cargo build --release
```

## Usage

```
ytui                         # opens with the search box
ytui lofi hip hop            # searches straight away
ytui https://youtube.com/…   # opens a video, playlist or channel URL
ytui --mode sextant --audio pulse
ytui --snapshot SRC --at 30  # print one rendered frame
```

Run `ytui --help` for all options and keys.

## Render modes (`m` cycles)

| Mode | Description |
|---|---|
| `ascii` | Two-tone shape-matched ASCII: flat areas are clean colour, glyphs only where there is edge detail |
| `classic` | Shape-matched ASCII, colour ink on a dark ground |
| `sextant` | 2×3 two-colour mosaic per cell (sharpest) |
| `quadrant` | 2×2 two-colour mosaic |
| `halfblock` | Top/bottom half blocks |
| `braille` | 2×4 dot mosaic |

The ASCII modes match glyph *shape*, not just brightness, following the technique in Alex Harri's
["ASCII characters are not pixels"](https://alexharri.com/blog/ascii-rendering): each glyph is
rasterised from a real monospace font and described by its ink coverage in six staggered sampling
circles; image cells are sampled the same way, with directional and global contrast enhancement,
and the nearest glyph wins.

## Player keys

space pause · ←/→ 10s · ↑/↓ 60s · 0–9 jump · m mode · b ascii background ·
`[ ]` global contrast · `{ }` edge contrast · n/p next/prev · q back

Audio goes through PulseAudio or ALSA (`--audio`); it is muted by default over SSH.

## License

MIT, see [LICENSE](LICENSE).
