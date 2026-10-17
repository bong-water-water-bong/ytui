---
title: Troubleshooting
description: Fixes for common ytui problems, such as YouTube's bot check, no pixel graphics in tmux, banded colours, libmpv not found and no audio over SSH.
---

## "Sign in to confirm you're not a bot"

YouTube has stopped serving videos to your connection for now. It usually clears up within a few
hours. To keep watching, [sign in for playback](../sign-in/#sign-in-for-playback-the-bot-check):
press `@` in ytui, or run `ytui --signin`.

## Videos don't play at all

Check that `yt-dlp` and `ffmpeg` are on your `PATH`, and that yt-dlp has a JavaScript runtime
(Deno, or Node.js 22+). Update yt-dlp first, since YouTube changes often:

```
yt-dlp -U
```

If yt-dlp lives somewhere else, set `YTUI_YTDLP` to its path.

## libmpv isn't found

ytui falls back to ffmpeg. That still plays, but without instant seek, captions drawn by mpv, or the native window. Install libmpv
(`libmpv2` or `mpv-libs` on Linux, `brew install mpv` on macOS). On Windows, put `libmpv-2.dll` next to
`ytui.exe` or in `%LOCALAPPDATA%\libmpv\`. Or point `YTUI_LIBMPV` at the library.

## No pixel graphics

- Check that your terminal supports [sixel or the kitty protocol](../pixel-graphics/#which-terminals-work).
- Inside tmux, the kitty protocol needs `set -g allow-passthrough on` in `~/.tmux.conf`.
- Make sure pixel graphics aren't switched off: press `G`, or start with `--gfx sixel` or `--gfx kitty`.

## Colours look banded or wrong under tmux

tmux doesn't know your terminal supports 24-bit colour. Add this to `~/.tmux.conf` and reattach:

```
set -as terminal-features ',xterm*:RGB'
```

## Sextant mode shows boxes

Your font doesn't have Unicode sextant characters. Switch to a font that does, or use `quadrant`
or `halfblock`.

## No sound over SSH

Audio is off by default over SSH. Choose an output with `--audio`, for example `--audio pulse`.

## Video is choppy over SSH

Pixel video over SSH is capped at 960 px and 12 fps, to stay around 4 MB/s. On a fast link, raise the caps
with `--gfx-px` and `--gfx-fps`. On a slow one, switch to a character mode with `G`.

## Still stuck?

[Open an issue](https://github.com/bong-water-water-bong/ytui/issues) with your operating system, your terminal, and what
ytui printed.
