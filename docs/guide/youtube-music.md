---
title: YouTube Music
description: Use YouTube Music in the terminal with ytui. Browse Home, Explore, charts, moods and genres, search songs, albums and artists, and play with a queue and lyrics.
---

**♫ Music** in the sidebar opens [YouTube Music](https://music.youtube.com/) inside ytui. It's the
music.youtube.com site rebuilt in the terminal, and it talks to YouTube Music's own service, so you see
the same shelves, charts and recommendations. Pasting a music.youtube.com link opens it there too.

![ytui's YouTube Music Home, signed out: Quick picks as three columns of songs, then Today's biggest hits as album-art cards]({{ '/screenshots/music-home.jpg' | relative_url }})

## Pages

The bar at the top of every Music page has **Home**, **Explore**, **Library** and **Search music**.

- **Home:** Quick picks, Today's biggest hits, mixes and more shelves, which keep loading as you
  scroll. Signed in, it also has your Listen again and your mixes.
- **Explore:** New releases, Charts, Moods & genres and Podcasts, then new albums and singles,
  top songs, trending and new music videos.
- **Search music:** results come as Top result, Songs, Videos, Albums, Artists and Playlists, each
  with **More ›** to see them all. The filter chips at the top do the same. While you're on a Music
  page, `/` searches YouTube Music.
- **Artists:** a banner, the description, **⇄ Shuffle**, **◉ Radio** and **Subscribe**, then top
  songs, albums, singles, videos and playlists they're featured on.
- **Albums and playlists:** the cover, details, **▶ Play** and **⇄ Shuffle**, then the full
  track list.
- **Library** (signed in): your playlists, liked songs, albums, artists, subscriptions and
  history.

![The album 25 Years (Volume 2) by Kevin MacLeod: its cover beside the title and Play and Shuffle buttons, then the numbered track list]({{ '/screenshots/music-album.jpg' | relative_url }})

## Playing music

Choose a song and the music player opens. The cover art or music video plays as the picture, with
the song, its artists and album (each a link), and **Like**, **Save** and **Later** buttons. Beside
it are three tabs:

- **Up next:** the queue. A song from an album or playlist plays through that list. A song from
  anywhere else starts a radio, as YouTube Music does. Choose any entry to jump to it.
- **Lyrics**, where YouTube Music has them, with their source.
- **Related:** songs and playlists like this one.

When a song ends, the next one in the queue starts. The playback keys are the same as for videos
(`space` pauses, `,` `.` seek 10 s, `+` `-` volume, `f` fullscreen).

![The music player paused on Breaktime by Kevin MacLeod: the album cover as the picture, the song and album below, and the album's songs in Up next]({{ '/screenshots/music-player.jpg' | relative_url }})

## Links

All of these open in ytui, from the address bar or the command line:

```
ytui https://music.youtube.com/
ytui https://music.youtube.com/explore
ytui "https://music.youtube.com/search?q=daft punk"
ytui https://music.youtube.com/watch?v=VIDEO_ID
ytui https://music.youtube.com/playlist?list=PLAYLIST_ID
ytui https://music.youtube.com/browse/ALBUM_ID
ytui https://music.youtube.com/channel/ARTIST_ID
```

The screenshots show *25 Years (Volume 2)* by [Kevin MacLeod](https://incompetech.com/), licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
