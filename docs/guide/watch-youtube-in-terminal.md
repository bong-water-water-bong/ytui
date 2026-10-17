---
title: How to watch YouTube in the terminal
description: A step-by-step guide to watching YouTube in a terminal on Windows, Linux and macOS, over SSH and inside tmux, with ASCII art or real pixels.
---

You can search and watch YouTube without leaving your terminal. This guide sets up
[ytui](../../), a free terminal YouTube client, on each system. Then it covers
the parts people usually get stuck on: SSH, tmux and picture quality.

## What you need

- A terminal with 24-bit colour. Almost every modern terminal has it.
- **yt-dlp**, which finds the video streams, plus Deno or Node.js 22+, which yt-dlp needs for YouTube.
- **ffmpeg**, and ideally **libmpv** for smooth 60 fps playback.
- For real pixels instead of characters, a terminal with sixel or the kitty graphics protocol (optional).

## On Windows

1. Install the tools:

   ```
   winget install yt-dlp.yt-dlp Gyan.FFmpeg DenoLand.Deno
   ```

2. Download `ytui-<version>-windows-x86_64.zip` from the
   [latest release](https://github.com/bong-water-water-bong/ytui/releases/latest) and unzip it.
   libmpv is included.
3. Open **Windows Terminal**, go to the folder and run `ytui.exe`.

Windows Terminal supports sixel, so thumbnails and video show as real pixels. ytui can also play
video in a [native GPU window](../pixel-graphics/#native-video-window-windows-terminal) that sits exactly
over the player, at up to 1080p60. Press `N` to switch to it.

## On Linux

1. Install `yt-dlp`, `ffmpeg`, Deno or Node.js 22+, and libmpv with your package manager. The
   libmpv package is usually called `libmpv2` (Debian, Ubuntu) or `mpv-libs` (Fedora). On Arch, it comes with `mpv`.
2. Download `ytui-<version>-linux-x86_64.tar.gz` from the
   [latest release](https://github.com/bong-water-water-bong/ytui/releases/latest), extract it, and
   put `ytui` somewhere on your `PATH`.
3. Run `ytui`.

For real pixels, use a terminal with the kitty protocol (kitty, Ghostty) or sixel (foot, WezTerm,
Konsole, xterm with `-ti vt340`).

## On macOS

```
brew install yt-dlp ffmpeg deno mpv
```

Download `ytui-<version>-macos-arm64.tar.gz` from the
[latest release](https://github.com/bong-water-water-bong/ytui/releases/latest), extract it and run
`ytui`. kitty, Ghostty and WezTerm all show real pixels.

## Find and play a video

- `ytui` opens YouTube's Home page. When you're signed out, it opens on Trending.
- Press `/` and type a search, or paste a YouTube URL. `Enter` plays the video.
- You can also start straight from your shell: `ytui lofi hip hop`, or `ytui https://youtu.be/…`.
- `space` pauses, `f` goes fullscreen, `,` and `.` seek 10 seconds back and forward, and `?` shows every key.

## Choose how it looks

Press `m` to cycle the [render modes](../render-modes/): ASCII art, glow, and sextant, quadrant,
half-block and braille mosaics. Press `G` to switch between characters and real pixels. Press `o`
to tune brightness, saturation and contrast.

![Big Buck Bunny drawn as ASCII art in a terminal by ytui]({{ '/screenshots/glow.jpg' | relative_url }})

## Watch YouTube over SSH

ytui works the same over SSH. The character modes need nothing but a truecolour terminal on
your end. A few things change automatically:

- **Audio is off** over SSH, because the sound would play on the remote machine. Turn it on with `--audio pulse`
  (or `alsa`) if you want sound to play there.
- **Pixel video is capped** at 960 px and 12 fps, to keep it around 4 MB/s. On a fast link, raise the caps
  with `--gfx-px 1920 --gfx-fps 30`.
- Over a slow link, press `G` to use characters, which need far less bandwidth.

## Watch YouTube inside tmux

- **Colours:** if tmux doesn't know your terminal supports 24-bit colour, everything looks banded. Add this
  to `~/.tmux.conf` and reattach:

  ```
  set -as terminal-features ',xterm*:RGB'
  ```

- **Pixels:** the kitty protocol works inside tmux once you add `set -g allow-passthrough on`.

## If YouTube says "confirm you're not a bot"

YouTube sometimes blocks a connection for a few hours. Press `@` in ytui (or run `ytui --signin`)
and sign in with Google in the window that opens. See [Signing in](../sign-in/).

## Next

- [Keys](../keys/) lists every shortcut.
- [Troubleshooting](../troubleshooting/) covers videos that won't play, missing libmpv and more.
- [Terminal YouTube clients compared](../alternatives/) shows how ytui differs from ytfzf, youtube-tui and others.
