---
title: Playback and audio
description: How ytui plays YouTube through ffmpeg at up to 60 fps, plus captions, audio output and volume.
---

## ffmpeg

Every video plays through **ffmpeg** and is drawn in the terminal. yt-dlp finds the streams, ffmpeg
decodes them, and ytui turns each frame into [braille or glow](../render-modes/) characters.

## Quality and frame rate

ytui fetches streams up to 720p by default. That's the lowest height YouTube serves at 60 fps, so
60 fps streams are chosen when a video has them.

`--height N` changes the maximum stream height. `--fps N` caps the frame rate (default 60).

## Captions

`S` cycles through off and each caption track, uploaded tracks first, then auto-generated ones. The
language you pick is remembered for later videos. ytui fetches the track's WebVTT file itself and
draws each caption as text over the video.

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
