---
title: ytui docs
description: How to install ytui, watch YouTube in your terminal, pick a render mode, and set up pixel graphics, sign-in and playback.
---

ytui is a YouTube client that runs in your terminal. It works like a browser for YouTube, with
tabs, an address bar, Home and Trending, search, channels, playlists, comments and live chat. It
plays video as ASCII art, as Unicode mosaics, or as real pixels with sixel and kitty graphics.

Browsing uses YouTube's own web API directly, with no API key and no account. yt-dlp is only used to find the
video streams, and libmpv (or ffmpeg) plays them.

## Start here

1. [Install ytui](install/) and the tools it plays through. The [step-by-step guide](watch-youtube-in-terminal/) covers each system, SSH and tmux.
2. Run `ytui`. It opens YouTube's Home page.
3. Press `/` to search, then `Enter` to play a video. Press `?` at any time to see every key.

## Find what you need

- [Using ytui](usage/): browsing, tabs, the watch page, live chat, and starting from the command line.
- [Render modes](render-modes/): ASCII art, glow, mosaics, the look panel and auto-exposure.
- [Pixel graphics](pixel-graphics/): sixel, kitty, the native window on Windows, the mpv window on Linux and macOS, and colours under tmux.
- [Playback and audio](playback/): libmpv or ffmpeg, captions, audio outputs and volume.
- [Signing in](sign-in/): getting past YouTube's bot check, and Google sign-in for likes, comments and playlists.
- [Keys](keys/) and [command-line options](options/).
- [Troubleshooting](troubleshooting/) for when something doesn't play or look right.
- [Terminal YouTube clients compared](alternatives/): ytui next to ytfzf, youtube-tui, yewtube and mpv.
