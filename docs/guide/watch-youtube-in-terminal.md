---
title: How to watch YouTube in the terminal
description: A step-by-step guide to watching YouTube in a terminal on Windows, Linux and macOS, over SSH and inside tmux, as 24-bit colour braille or glow ASCII.
---

You can search and watch YouTube without leaving your terminal. This guide sets up
[ytui](../../), a free terminal YouTube client, on each system. Then it covers
the parts people usually get stuck on: SSH, tmux and picture quality.

## What you need

- A terminal with 24-bit colour. Almost every modern terminal has it.
- **yt-dlp**, which finds the video streams, plus Deno or Node.js 22+, which yt-dlp needs for YouTube.
- **ffmpeg**, which plays the video.

## On Windows

1. Install the tools:

   ```
   winget install yt-dlp.yt-dlp Gyan.FFmpeg DenoLand.Deno
   ```

2. Download `ytui-<version>-windows-x86_64.zip` from the
   [latest release](https://github.com/bong-water-water-bong/ytui/releases/latest) and unzip it.
3. Open **Windows Terminal**, go to the folder and run `ytui.exe`.

Audio plays through `ffplay`, which comes with ffmpeg.

## On Linux

1. Install `yt-dlp`, `ffmpeg`, and Deno or Node.js 22+ with your package manager.
2. Download `ytui-<version>-linux-x86_64.tar.gz` from the
   [latest release](https://github.com/bong-water-water-bong/ytui/releases/latest), extract it, and
   put `ytui` somewhere on your `PATH`.
3. Run `ytui`.

## On macOS

```
brew install yt-dlp ffmpeg deno
```

Download `ytui-<version>-macos-arm64.tar.gz` from the
[latest release](https://github.com/bong-water-water-bong/ytui/releases/latest), extract it and run
`ytui`.

## Find and play a video

- `ytui` opens YouTube's Home page. When you're signed out, it opens on Trending.
- Press `/` and type a search, or paste a YouTube URL. `Enter` plays the video.
- You can also start straight from your shell: `ytui lofi hip hop`, or `ytui https://youtu.be/…`.
- `space` pauses, `f` goes fullscreen, `,` and `.` seek 10 seconds back and forward, and `?` shows every key.

## Choose how it looks

Press `m` to switch between the two [render modes](../render-modes/): glow (the default) and
braille. Press `o` to tune brightness, saturation, background and contrast.

![Big Buck Bunny under an apple tree, drawn as coloured ASCII characters in glow mode, fullscreen]({{ '/screenshots/glow.jpg' | relative_url }})

## Watch YouTube over SSH

ytui works the same over SSH. It needs nothing but a truecolour terminal on your end.
One thing changes automatically:

- **Audio is off** over SSH, because the sound would play on the remote machine. Turn it on with `--audio pulse`
  (or `alsa`) if you want sound to play there.

## Watch YouTube inside tmux

ytui only draws in 24-bit colour. If tmux doesn't know your terminal supports it, everything looks
banded and ytui shows a hint. Add this to `~/.tmux.conf` and reattach:

```
set -as terminal-features ',xterm*:RGB'
```

## If YouTube says "confirm you're not a bot"

YouTube sometimes blocks a connection for a few hours. Press `@` in ytui (or run `ytui --signin`)
and sign in with Google in the window that opens. See [Signing in](../sign-in/).

## Next

- [Keys](../keys/) lists every shortcut.
- [Troubleshooting](../troubleshooting/) covers videos that won't play, banded colours and more.
- [Terminal YouTube clients compared](../alternatives/) shows how ytui differs from ytfzf, youtube-tui and others.
