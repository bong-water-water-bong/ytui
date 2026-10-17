---
title: Command-line options
description: All ytui command-line options and environment variables, for audio, stream quality and cookies.
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
| `ytui --uninstall [--yes]` | Sign out and remove ytui, its settings, data and cache ([details](../install/#uninstall)) |

## Options

| Option | What it does |
|---|---|
| `--audio A` | Audio output: `auto`, `pulse`, `alsa`, `ffplay` or `none`. `auto` uses PulseAudio when it's there and `ffplay` on Windows. Off by default over SSH. |
| `--fullscreen` | Start the player fullscreen |
| `--fps N` | Maximum frame rate (default 60) |
| `--height N` | Maximum stream height to fetch (default 720, the lowest height YouTube serves at 60 fps) |
| `--exposure X` | `on` or `off`: auto-exposure for `--snapshot` (in the app, press `E`) |
| `--cookies FILE` | A `cookies.txt` for yt-dlp (default: `cookies.txt` in the config directory, if present) |
| `--cookies-from-browser B` | Read cookies from browser B (Firefox works; Chrome and Edge usually can't be read while running) |

## Environment variables

| Variable | What it does |
|---|---|
| `YTUI_YTDLP` | Path to yt-dlp |
| `YTUI_BROWSER` | The browser to sign in with (a path, or a name like `firefox`) |
| `XDG_DATA_HOME`, `XDG_CONFIG_HOME` | Where ytui keeps its data and settings |
