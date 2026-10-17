---
title: Command-line options
description: All ytui command-line options and environment variables, for render modes, audio, the playback engine, pixel graphics, colours and cookies.
---

`ytui --help` prints this list.

## Commands

| Command | What it does |
|---|---|
| `ytui` | Open Home |
| `ytui QUERY...` | Open a search |
| `ytui URL` | Open a video, playlist or channel URL |
| `ytui --snapshot SRC [--at SECONDS] [--cols N --rows N]` | Print one rendered frame of a URL or file, then exit |
| `ytui --import-subs FILE` | Import a Google Takeout `subscriptions.csv` |
| `ytui --signin` | Sign in for playback when YouTube asks you to confirm you're not a bot |
| `ytui --signout-cookies` | Forget the playback sign-in |
| `ytui --help-cookies` | Explain the playback sign-in |
| `ytui --login [--client FILE]` | Sign in with Google to like and comment |
| `ytui --logout` | Sign out and revoke access |
| `ytui --help-login` | How to create the Google OAuth client |
| `ytui --demo3d [SCENE]` | 3D engine showcase: `logo`, `knot`, `terrain` or `visualizer` |
| `ytui --uninstall [--yes]` | Sign out and remove ytui, its settings, data and cache ([details](../install/#uninstall)) |

## Options

| Option | What it does |
|---|---|
| `--mode M` | Render mode: `glow`, `art`, `ascii`, `classic`, `sextant`, `quadrant`, `halfblock` or `braille` |
| `--audio A` | Audio output: `auto`, `pulse`, `alsa`, `ffplay` or `none`. Off by default over SSH. |
| `--engine E` | Playback engine: `auto`, `mpv` or `ffmpeg`. `auto` uses libmpv when it's found. |
| `--no-native` | Never use the native video window (Windows) |
| `--fullscreen` | Start the player fullscreen |
| `--fps N` | Maximum frame rate with the ffmpeg engine (default 60) |
| `--height N` | Maximum stream height to fetch (default 720 in the terminal; the window's height for native video) |
| `--exposure X` | `on` or `off`: auto-exposure for `--snapshot` (in the app, press `E`) |
| `--gfx X` | Pixel graphics: `auto`, `kitty`, `sixel` or `off` |
| `--gfx-fps N` | Cap pixel video at N fps (default 12 over SSH, no limit on your own computer) |
| `--gfx-px N` | Longest edge of pixel images |
| `--sixel-colors N` | Colours per sixel frame, 2–255 (default 255) |
| `--colors C` | Colour depth: `auto`, `256` or `true` |
| `--cookies FILE` | A `cookies.txt` for yt-dlp |
| `--cookies-from-browser B` | Read cookies from browser B (Firefox works; Chrome and Edge usually can't be read while running) |

## Environment variables

| Variable | What it does |
|---|---|
| `YTUI_YTDLP` | Path to yt-dlp |
| `YTUI_LIBMPV` | Path to libmpv |
| `YTUI_ENGINE` | Same as `--engine` |
| `YTUI_GFX` | Same as `--gfx` |
| `YTUI_COLORS` | Same as `--colors` |
| `YTUI_FONT` | A monospace `.ttf` whose character shapes the ASCII modes match against |
| `YTUI_BROWSER` | The browser to sign in with |
| `YTUI_SIXEL_EXACT` | Set to `1` to resend every sixel row that changed |
| `XDG_DATA_HOME`, `XDG_CONFIG_HOME` | Where ytui keeps its data and settings |
