---
title: Install ytui
description: Download ytui for Windows, Linux or macOS, install yt-dlp, ffmpeg and libmpv, or build it from source with Cargo.
---

## Download a release

Get the package for your system from the
[latest release](https://github.com/bong-water-water-bong/ytui/releases/latest).

| System | Package |
|---|---|
| Windows 10 and 11 (x64) | `ytui-<version>-windows-x86_64.zip`. libmpv is included. |
| Linux (x86_64) | `ytui-<version>-linux-x86_64.tar.gz` |
| macOS (Apple Silicon) | `ytui-<version>-macos-arm64.tar.gz` |

Each release also has `SHA256SUMS.txt` so you can check your download.

## Install what ytui plays through

ytui needs these on your `PATH`:

- **yt-dlp**, which finds the video streams. YouTube needs yt-dlp to have a JavaScript runtime, so also install **Deno** or **Node.js 22+**.
- **ffmpeg**.
- **libmpv**, optional but recommended. It's the playback engine: captions, instant seek and pause, A/V sync,
  hardware decoding, and the native video window on Windows. Without it, ytui plays through ffmpeg.

### Windows

```
winget install yt-dlp.yt-dlp Gyan.FFmpeg DenoLand.Deno
```

Unzip the release and run `ytui.exe` in **Windows Terminal**. That gives you sixel graphics and
the native video window. The release zip already has `libmpv-2.dll` next to `ytui.exe`.

### macOS

```
brew install yt-dlp ffmpeg deno mpv
```

### Linux

Install `yt-dlp`, `ffmpeg`, Deno or Node.js 22+, and libmpv (the package is usually called
`libmpv2` or `mpv-libs`) with your package manager.

You need a truecolour terminal. For the `sextant` render mode, also use a font that has Unicode sextant characters.

## Build from source

```
git clone https://github.com/bong-water-water-bong/ytui
cd ytui
cargo build --release
```

The binary is `target/release/ytui`.

On Windows, for a source build, put `libmpv-2.dll` from a
[shinchiro mpv-dev build](https://github.com/shinchiro/mpv-winbuild-cmake/releases) next to
`ytui.exe` or in `%LOCALAPPDATA%\libmpv\`, or point `YTUI_LIBMPV` at it.

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
- the `ytui` program itself, plus the files that came with it in the release folder (`libmpv-2.dll` and the licences)

If you're signed in with Google, it also signs you out, which revokes ytui's access to your account. yt-dlp, ffmpeg and mpv
stay installed, because other programs may use them. Add `--yes` to skip the question, for example in a script.
