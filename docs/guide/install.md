---
title: Install ytui
description: Download ytui for Windows, Linux or macOS, install yt-dlp and ffmpeg, or build it from source with Cargo.
---

## Download a release

Get the package for your system from the
[latest release](https://github.com/bong-water-water-bong/ytui/releases/latest).

| System | Package |
|---|---|
| Windows 10 and 11 (x64) | `ytui-<version>-windows-x86_64.zip` |
| Linux (x86_64) | `ytui-<version>-linux-x86_64.tar.gz` |
| macOS (Apple Silicon) | `ytui-<version>-macos-arm64.tar.gz` |

Each package holds `ytui` (or `ytui.exe` on Windows), `README.md`, `LICENSE` and `NOTICE`.
Each release also has `SHA256SUMS.txt` so you can check your download.

## Install what ytui plays through

ytui needs these on your `PATH`:

- **yt-dlp**, which finds the video streams. YouTube needs yt-dlp to have a JavaScript runtime, so also install **Deno** or **Node.js 22+**.
- **ffmpeg**, which plays every video.

### Windows

```
winget install yt-dlp.yt-dlp Gyan.FFmpeg DenoLand.Deno
```

Unzip the release and run `ytui.exe` in **Windows Terminal**. Audio plays through `ffplay`,
which comes with ffmpeg.

### macOS

```
brew install yt-dlp ffmpeg deno
```

### Linux

Install `yt-dlp`, `ffmpeg`, and Deno or Node.js 22+ with your package manager.

You need a truecolour terminal.

## Build from source

```
git clone https://github.com/bong-water-water-bong/ytui
cd ytui
cargo build --release
```

The binary is `target/release/ytui`.

## Where ytui keeps things

Subscriptions, history, watch later, settings and sign-in are stored in `~/.local/share/ytui/`
(`%USERPROFILE%\.local\share\ytui\` on Windows). Set `XDG_DATA_HOME` and `XDG_CONFIG_HOME` to
keep them somewhere else.

## Uninstall

```
ytui --uninstall
```

ytui first lists everything it will remove and asks before deleting anything:

- your settings and playback sign-in cookies (`~/.config/ytui/`)
- your history, subscriptions, watch later, Google sign-in and ytui's sign-in browser profile (`~/.local/share/ytui/`)
- its cache (`~/.cache/ytui/`)
- the `ytui` program itself, plus the files that came with it in the release folder (`README.md`, `LICENSE` and `NOTICE`)

If you're signed in with Google, it also signs you out, which revokes ytui's access to your account. yt-dlp and ffmpeg
stay installed, because other programs may use them. Add `--yes` to skip the question, for example in a script.
