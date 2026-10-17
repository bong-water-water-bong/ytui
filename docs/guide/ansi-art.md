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
| `ansi` | CP437 glyphs from the IBM 8×16 ROM font on the DOS palette. With iCE colours on, the background may use the high bit, giving 8 foregrounds × 16 backgrounds. |
| `ansiblink` | The same, with backgrounds limited to the 8 that do not blink. |
| `ansi9` | The same at 9 dots per cell: the extra column repeats the last dot of block and box characters, so rules join without a seam. |

These are the slow modes. Every CP437 glyph is fitted for every cell — there is no font atlas and no
shortlist — which is roughly 30 ms a frame at 120×40. They are best for stills, the exporter, and
`--demo3d`, where the 3D engine's output goes through the same shape-matched path. For live video the
mosaic modes (`sextant`, `quadrant`, `halfblock`, `braille`) are several hundred times faster.

## The commands

```
ytui --view-ans art.ans                  show art in the terminal
ytui --snapshot-ans VIDEO -o art.ans     save one video frame as ANSI art
ytui --convert-ans art.ans -o out.ans    re-encode existing art
```

`--snapshot-ans` takes the same `--at SECONDS`, `--cols` and `--rows` as `--snapshot`. It defaults to
80×25, the geometry the art was drawn to.

`--view-ans` re-emits the art normalised — every row placed with an absolute cursor move, so it does
not depend on how your client handles wrapping at column 80 — while keeping the picture identical.

Options for anything that writes a file:

| Option | Effect |
|---|---|
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
