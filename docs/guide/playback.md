---
title: Playback and audio
description: How ytui plays YouTube through ffmpeg at up to 60 fps, its YouTube-style player controls, captions, audio output and volume.
---

## ffmpeg

Every video plays through **ffmpeg** and is drawn in the terminal. yt-dlp finds the streams, ffmpeg
decodes them, at exactly the size of the terminal's cell grid, and ytui draws each frame as
[truecolour half blocks](../render-modes/).

## Quality and frame rate

ytui fetches streams up to 720p by default. That's the lowest height YouTube serves at 60 fps, so
60 fps streams are chosen when a video has them.

`--height N` changes the maximum stream height. `--fps N` caps the frame rate (default 60).

## Captions

The first press of `S` picks your system language (from `LANG`), or English if there's none,
and prefers the creator's captions over automatic ones. Pressing it again goes through each track,
then off. The language you pick is remembered for later videos. ytui fetches the track's WebVTT
file itself and draws each caption as bold white text in a solid black box, with a blank row above
and below.

![Sintel fullscreen in a firelit close-up, with the English caption “You’re a fool for traveling alone, so completely unprepared.” in a black box, and the player controls below it]({{ '/screenshots/captions.jpg' | relative_url }})

## Player controls

Move the mouse over the video, or press a player key, and a control row shows on the video's bottom line,
as on YouTube: previous, play/pause, next and volume on the left, captions (`CC`) and fullscreen on the
right. It stays up while the video is paused and hides three seconds after the last movement. Click the
volume to mute. Previous goes back to the start, or to the previous video in a playlist or album when
you're in its first five seconds. The progress bar, time and volume slider stay on the player bar below.

## Audio

ytui plays audio through PipeWire or PulseAudio (`pw-cat` or `pacat`), or through ALSA (`aplay`),
and sets the volume itself. On Windows it uses `ffplay`, which comes with ffmpeg. On Linux and macOS,
if none of those are there, ffmpeg plays the audio directly.

`--audio auto|pulse|alsa|ffplay|none` picks the output. Audio is off by default over SSH.

Volume goes from 0 to 150%. Use `+` and `-`, `M` to mute, or the slider on the player bar. Volume changes
take effect right away, without restarting the video.

## Why there's a local relay

YouTube stops sending data on open-ended downloads after about 800 KB. So ffmpeg
reads the video through a small local relay in ytui, which fetches it in chunks of a fixed size.
