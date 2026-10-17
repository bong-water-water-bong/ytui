---
title: Terminal YouTube clients compared
description: How ytui compares with ytfzf, youtube-tui, yewtube and mpv for watching YouTube from the terminal, and which to pick for what you want.
---

There are several good ways to use YouTube from a terminal. Most of them help you *find* a
video in the terminal, then hand it to mpv to *play* in a separate window. ytui is built around
playing the video inside the terminal itself, in a YouTube-style interface.

This page describes how each tool usually works by default. All of them are open source and change over time,
so check each project for its current features.

## At a glance

| Tool | What it is | Where video plays |
|---|---|---|
| **ytui** | A full-screen YouTube client, with Home, search, channels, comments and live chat | Inside the terminal, as 24-bit colour braille or glow ASCII |
| **ytfzf** | A shell script that searches YouTube and lets you pick results with fzf | Hands the video to mpv (or another player) |
| **youtube-tui** | A TUI for browsing YouTube | Hands the video to mpv |
| **yewtube** (formerly mps-youtube) | A command-line player for searching, playlists and music | Hands playback to mpv or mplayer. Often used for audio. |
| **mpv** with yt-dlp | A video player that can play YouTube URLs directly | In its own window, or in the terminal with `--vo=tct`, `--vo=sixel` or `--vo=kitty` |

## When ytui fits

- You want **the video in the terminal**: over SSH, in a tiling setup, or just because you like the look.
- You want a **YouTube-like interface**, with Home and Trending, channels, playlists, comments, live chat and
  captions, without opening a browser.
- You're on **Windows**. ytui runs natively in Windows Terminal.
- You want **picture control** over how the video turns into characters: [two render modes](../render-modes/),
  a look panel and auto-exposure.

## When something else fits better

- **You only want a quick search, then a normal video window:** ytfzf or youtube-tui with mpv is lightweight and fast.
- **You mostly listen to music:** yewtube is built around audio and playlists.
- **You already have a URL and just want to play it:** `mpv <url>` (with yt-dlp installed) is the shortest path.

## Can I use ytui together with these?

Yes. They all use yt-dlp underneath, so they don't conflict. Updating yt-dlp (`yt-dlp -U`) helps all of them
when YouTube changes something.

[Install ytui](../install/) or read [how to watch YouTube in the terminal](../watch-youtube-in-terminal/).
