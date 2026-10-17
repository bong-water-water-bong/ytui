---
title: Troubleshooting
description: Fixes for common ytui problems, such as YouTube's bot check, banded colours under tmux and no audio over SSH.
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

## Colours look banded or wrong under tmux

ytui only draws in 24-bit colour, and tmux doesn't know your terminal supports it. tmux then rounds
every colour to 256, and ytui shows a hint. Add this to `~/.tmux.conf` and reattach:

```
set -as terminal-features ',xterm*:RGB'
```

## No sound over SSH

Audio is off by default over SSH. Choose an output with `--audio`, for example `--audio pulse`.

## Still stuck?

[Open an issue](https://github.com/bong-water-water-bong/ytui/issues) with your operating system, your terminal, and what
ytui printed.
