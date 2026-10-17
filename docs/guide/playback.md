---
title: Playback and audio
description: How ytui plays YouTube with libmpv or ffmpeg at up to 60 fps, plus captions, audio output and volume.
---

## libmpv or ffmpeg

With **libmpv** installed, mpv decodes the video, using hardware decoding where it can. It also keeps audio and video in step and
plays the audio and captions. ytui draws the frames it gets back. Seeking takes about a tenth of a second.

Without libmpv, ytui plays through an **ffmpeg** pipe instead.

`--engine mpv` or `--engine ffmpeg` picks one yourself (or set `YTUI_ENGINE`). If libmpv isn't
found, point `YTUI_LIBMPV` at it.

## Quality and frame rate

ytui chooses streams for where the video is drawn:

- In the terminal it fetches up to 720p, so 60 fps streams are chosen. YouTube only has 60 fps at 720p and up.
- The native window on Windows fetches up to its own pixel size.
- With mpv, AV1 is preferred, because it looks better at the same size.

`--height N` changes the maximum stream height. `--fps N` caps the frame rate with the ffmpeg engine.

## Captions

`S` cycles through off and each caption track, uploaded tracks first, then auto-generated ones. The
language you pick is remembered for later videos. In character modes, captions are drawn as text over
the video. With pixel graphics or the native window, mpv draws them into the picture.

## Audio

With libmpv, audio goes through mpv's own output: WASAPI on Windows, PipeWire or PulseAudio, or ALSA.
Without it, ytui plays audio through PipeWire or PulseAudio (`pw-cat` or `pacat`), or through ALSA (`aplay`), and sets the
volume itself. If none of those are there, ffmpeg plays the audio directly.

`--audio auto|pulse|alsa|ffplay|none` picks the output. Audio is off by default over SSH.

Volume goes from 0 to 150%. Use `+` and `-`, `M` to mute, or the slider on the player bar. Volume changes
take effect right away, without restarting the video.

## Why there's a local relay

YouTube stops sending data on open-ended downloads after about 800 KB. So ffmpeg and mpv
read the video through a small local relay in ytui, which fetches it in chunks of a fixed size.
