---
title: ANSI art
description: Watch and write real text-mode art — CP437, the 16-colour DOS palette, and the iCE convention — plus three render modes that draw video in it.
---

ytui speaks the format the BBS art scene has used since 1990: 80×25 text mode, code page 437, and
the 16-colour attribute byte with its special cases. Three render modes draw video in it, and three
commands read and write it.

## Render modes

| Mode | What it looks like |
|---|---|
| `ansi` | CP437 glyphs from the IBM 8×16 ROM font in 24-bit colour: two colours per cell, the best two for the picture. |
| `ansi9` | The same at 9 dots per cell: the extra column repeats the last dot of block and box characters, so rules join without a seam. |
| `ansi-16` | The period-correct version — the 16-colour DOS palette, with iCE colours giving 8 foregrounds × 16 backgrounds. |
| `ansi9-16` | `ansi-16` at 9 dots per cell. |
| `ansiblink` | `ansi-16` with backgrounds limited to the 8 that do not blink. |

Truecolour is not a compromise here, it is the faster path. Once the glyph's ink mask is fixed, the
two colours that minimise squared error are simply the mean of the inked pixels and the mean of the
rest — no palette to search, and the error is derivable from channel sums so the cell is not walked
again per glyph. That compares with the palette fit, which has to round two means to a 16-entry
table. On a real frame the truecolour reconstruction comes out 99.8% closer to the source.

## How fast they are

Every CP437 glyph is fitted for every cell, so cost scales with the terminal:

| Terminal | per frame | |
|---|---|---|
| 80×24 | ~33 ms | ~30 fps, fine |
| 120×40 | ~79 ms | ~13 fps, noticeable |
| 200×50 | ~165 ms | ~6 fps, unusable |

ytui warns when you switch to one of these modes and the window is large enough that it will not
keep up. The mosaic modes (`sextant`, `quadrant`, `halfblock`, `braille`) are sub-millisecond at any
size and stay the right choice for live video.

A note on why there is no clever optimisation here: two were tried and both were rejected on
measurement. A glyph shortlist reached 29 ms but cost 34% more paint error on real footage, and a
temporal reuse cache never found a threshold that worked, because the best-fit error varies 300×
across the cells of a single frame.

## The commands

```
ytui --view-ans art.ans                  show art in the terminal
ytui --snapshot-ans VIDEO -o art.ans     save one video frame as ANSI art (24-bit)
ytui --convert-ans art.ans -o out.ans    re-encode existing art
```

`--snapshot-ans` takes the same `--at SECONDS`, `--cols` and `--rows` as `--snapshot`. It defaults to
80×25, the geometry the art was drawn to.

`--view-ans` re-emits the art normalised — every row placed with an absolute cursor move, so it does
not depend on how your client handles wrapping at column 80 — while keeping the picture identical.

Options for anything that writes a file:

| Option | Effect |
|---|---|
| `--palette` | Write the 16-colour DOS version instead of 24-bit. Much smaller files. |
| `--no-ice` | Read the background high bit as blink, so backgrounds are limited to 8. |
| `--compact` | Separate rows with CR/LF instead of absolute moves. Smaller files, but a client that wraps at column 80 will put a blank line between every row. |
| `--bits 9` | Apply the 9th-dot rule, and record it in the SAUCE record. |

## What makes this format awkward

Four things bite anyone writing a reader, and each one is a wrong picture rather than an error:

**`ESC[1m` is not bold.** It sets the foreground attribute's high bit, so it brightens 30–37 into
90–97. It brightens the colour *already set*, not only the next one, and art writes a bare `ESC[1m`
straight after a colour to make it bright. Treat it as a font weight and the whole picture comes out a
shade too dark.

**`ESC[5m` is not blink.** Same idea for the background attribute's high bit. Whether that bit is a
colour depends on the iCE convention, and that is a *reading* choice, not something the file records:
the same bytes render differently depending on which era's rules you apply. SAUCE's flag byte can say,
and when it does, ytui follows it.

**The DOS and ANSI colour orders disagree.** DOS orders an attribute byte so blue is bit 0 and red is
bit 2, which makes attribute 1 blue and attribute 4 red. ANSI orders `ESC[31m` red and `ESC[34m` blue.
Every conversion has to go through the permutation once.

**Truecolour has to be written last.** 24-bit colour goes into the file as PabloDraw's
`ESC[1;r;g;bt` (foreground) and `ESC[0;r;g;bt` (background) sequences, *after* the palette escape for
the same cell. A viewer that understands them shows the exact colour; one that ignores them still
draws the right 16-colour picture instead of nothing. AnsiLove does understand them, and switches its
own renderer to RGB when it sees one.

**Bright has one spelling that everyone reads.** `ESC[1;3Xm`, not the modern `ESC[9Xm`: AnsiLove, the
reference implementation, does not implement 90–97 at all. And a foreground escape must not begin with
`ESC[0m`, because that also resets the background you just set.

## How it is tested

Against [AnsiLove/C](https://www.ansilove.org), the reference implementation, in both directions:

* AnsiLove's own `bs-alove.ans` fixture renders **pixel for pixel identically** to the reference
  render — 604 160 pixels, zero differences.
* Our re-encoding of that file, rendered by AnsiLove, also matches exactly.

Both tests ship in the repo and decode the reference PNGs in-process, so they need no external
tooling. They are not decorative: reverting the `ESC[1m` fix makes the first one fail on 35 376 pixels.
