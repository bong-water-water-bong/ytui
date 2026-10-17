---
title: Using ytui
description: Browse YouTube in the terminal with tabs, search, channels, playlists, comments and live chat, and start ytui from the command line.
---

![ytui's Home page showing the Trending feed as a grid of thumbnails]({{ '/screenshots/home.jpg' | relative_url }})

## Start from the command line

```
ytui                          # opens Home
ytui lofi hip hop             # opens a search
ytui https://youtube.com/…    # opens a video, playlist or channel URL (also @handle)
ytui --mode sextant --audio pulse
ytui --import-subs subscriptions.csv    # import a Google Takeout export
ytui --snapshot SRC --at 30   # print one rendered frame and exit
```

See [command-line options](../options/) for everything else.

## Browse like a browser

- **Tabs** with back and forward history. The **address bar** shows live search suggestions and also takes URLs.
- **Home** shows "For you", which is built from your watch history and subscriptions. It also has chips for Trending, Music, Gaming, News, Live,
  Sports, Learning, Podcasts and Movies. When you're signed out and haven't watched anything yet, it opens on Trending.
- **Search** has filters for videos, shorts, channels, playlists, live and newest, and keeps loading more as you scroll.
- **Channel pages** show the banner and avatar, with Home, Videos, Shorts, Live and Playlists tabs.
- **Subscriptions, history and watch later** are stored on your computer. History remembers where you stopped in each video.
- **Mouse:** click anything, scroll with the wheel, and click the progress bar to seek.

## The watch page

- An inline player, the description with clickable chapter timestamps, likes and views.
- Comments, with replies.
- "Up next" with autoplay (`a` toggles it), and a queue when you play a playlist.
- **Live chat** on live streams, and **chat replay** on past streams, kept in step with the video.
  It also shows beside the video in fullscreen. `C` shows or hides it.
- **Miniplayer:** when you scroll or browse away, the video keeps playing in a corner, with controls to go back, close,
  and play or pause. `f` goes fullscreen.
- **Volume:** 0–150% and mute. The volume changes right away, without restarting the video.

## 3D visualiser

`V` swaps the video for a 3D audio visualiser driven by the real audio. A spinning 3D logo shows
while a video loads. `ytui --demo3d` shows the 3D engine on its own:

```
ytui --demo3d            # press 1-4 for logo, knot, terrain, visualizer
ytui --demo3d terrain    # m cycles render mode, arrows orbit, +/- zoom, space pause
```

The 3D engine is a small real-time software renderer. Its output goes through the same
shape-matched glyph renderer as video.
