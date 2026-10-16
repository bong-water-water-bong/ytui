# ytui

YouTube in the terminal: search, browse, and watch videos rendered as truecolour ASCII / Unicode mosaics.

## Requirements

- `yt-dlp` (plus Node.js 22+ or Deno, which yt-dlp needs for YouTube)
- `ffmpeg`
- a truecolour terminal; a font with Unicode sextants for `sextant` mode

## Build

```
cargo build --release
```

## Usage

```
ytui                         # opens Home
ytui lofi hip hop            # opens a search
ytui https://youtube.com/…   # opens a video, playlist or channel URL (also @handle)
ytui --mode sextant --audio pulse
ytui --import-subs subscriptions.csv   # Google Takeout export
ytui --snapshot SRC --at 30  # print one rendered frame
```

It works like a browser for YouTube:

- **Tabs** with back/forward history, and an **address bar** with live search suggestions
  that also takes URLs
- **Home**: "For you" built from your watch history and subscriptions, plus topic chips
- **Search** with filters (videos, shorts, channels, playlists, live, newest) and infinite scroll
- **Watch page**: inline player, description with clickable chapter timestamps, likes/views,
  comments with replies, "Up next" with autoplay, playlist queue
- **Live chat** on live streams and **chat replay** on past streams, synced to the playhead
  (also beside the video in fullscreen)
- **Volume control** (0–150%, mute), applied live without restarting playback
- **3D audio visualiser** (`V`) in place of the video, driven by the real audio, and a
  3D scene while videos load; `ytui --demo3d` shows the engine on its own
- **Miniplayer** when you scroll or browse away, with overlay controls (↗ back to the
  video, ✕ close, play/pause), and **fullscreen** (`f`)
- **Channel pages** (banner, avatar, Home/Videos/Shorts/Live/Playlists tabs) and **playlists**
- **Subscriptions, history (with resume positions) and watch later**, stored locally in
  `~/.local/share/ytui/`
- **Mouse**: click anything, wheel to scroll, click the progress bar to seek

- **Sign in** (optional) to like / dislike videos, comment and reply, send live chat
  messages, save videos to playlists and manage your playlists, and sync subscriptions with
  your YouTube account, using Google's official OAuth
  and the YouTube Data API (see below)

Browsing uses YouTube's own web JSON API directly (no API key, no account); `yt-dlp` is only used
to resolve stream URLs. Press `?` in the app for every key.

## Pixel graphics (kitty / sixel)

On terminals with a graphics protocol, ytui shows real pixels for video, thumbnails, avatars,
banners and the 3D scenes instead of characters. It asks the terminal at startup and picks:

- **kitty graphics protocol**: kitty, Ghostty (and kitty / Ghostty behind tmux). Images are
  sent zlib-compressed and drawn with Unicode placeholders, so they scroll and clip with the UI
  and work inside tmux (needs `set -g allow-passthrough on` in tmux).
- **sixel**: foot, WezTerm, xterm (`-ti vt340`), mlterm, Konsole, Windows Terminal and others.
  Fixed 252-colour palette with ordered dithering.
- otherwise the character renderers as before.

`G` switches between pixels and characters (saved). `--gfx auto|kitty|sixel|off` (or
`YTUI_GFX`) overrides detection. Over SSH, pixel video is capped at 640 px and 12 fps to keep
bandwidth around 4 MB/s; change with `--gfx-px N` and `--gfx-fps N` (0 = unlimited).

## Colours under tmux

ytui draws in 24-bit colour. If tmux doesn't know the outer terminal supports it, tmux rounds
every colour to its 256-colour palette, which bands gradients and shifts hues. ytui detects this
and maps the picture to the 256 palette itself with ordered dithering, and the status bar says
how to get full colour: add

```
set -as terminal-features ',xterm*:RGB'
```

to `~/.tmux.conf` and reattach. `--colors auto|256|true` (or `YTUI_COLORS`) overrides.

## 3D ASCII engine

ytui includes a small real-time 3D software renderer whose output goes through the same
shape-matched glyph renderer as video: multi-threaded triangle rasterisation with a depth buffer,
sun / ambient / rim lighting, specular highlights, shadow mapping, fog, procedural grid-line
floors, and an ink-outline pass on depth and normal edges so silhouettes come out as crisp
strokes. For 3D it crushes darks (empty space stays empty) and takes glyph density from colour
value, so saturated objects still read as solid.

```
ytui --demo3d            # logo · press 1-4 for logo, knot, terrain, visualizer
ytui --demo3d terrain    # m cycles render mode, arrows orbit, +/- zoom, space pause
```

In the app, `V` swaps the video for a 3D audio visualiser (an FFT of the audio, which also works
with sound off), and a spinning 3D logo shows while a video loads. At 180×49 cells the engine
renders 1080×588 px in 9–14 ms per frame on a 16-thread CPU.

## Signing in (likes and comments)

Browsing needs no account. To like, dislike, comment and reply, sign in with Google's official
OAuth flow using your own free Google Cloud OAuth client. ytui never sees your password; tokens
are stored in `~/.local/share/ytui/oauth.json` (mode 0600).

1. In the [Google Cloud console](https://console.cloud.google.com/), create a project and enable
   **YouTube Data API v3** (APIs & Services → Library).
2. Google Auth Platform → Branding: set an app name and your email. Audience: **External**, and
   add your Google account under **Test users**.
3. Google Auth Platform → Clients → Create client → **Desktop app** → download the JSON.
4. Run `ytui --login --client ~/Downloads/client_secret_XXXX.json` and open the link it prints.
   On a remote/SSH session the final page won't load (it points at `127.0.0.1`); copy that page's
   address from the browser and paste it into the terminal.

Signed in, your YouTube subscriptions are synced at startup and on the Subscriptions page, and
Subscribe / Unsubscribe change them on YouTube. Channels you subscribed to in ytui before
signing in stay local until you press "Upload … to YouTube" there. Live chat gets a
"Say something…" box (or press `I`).

**Playlists**: `≡+ Save` on a video (or `P` on any focused video) opens *Save to…* with your
playlists ticked where the video already is; tick/untick to add/remove, or create a new one.
The **Playlists** page in the sidebar lists your playlists; each one has Play all, Rename,
Privacy (private → unlisted → public), Delete, and ↑ ↓ ✕ on every video. YouTube's own
"Watch later" isn't available through the API, so ytui's Watch later stays local.

While the OAuth app is in *Testing*, Google expires the sign-in after 7 days (run `ytui --login`
again, or publish the app). The free quota is 10,000 units/day; a like or comment costs 50.
`ytui --logout` revokes access. `ytui --help-login` prints these steps.

## Render modes (`m` cycles)

| Mode | Description |
|---|---|
| `art` | Clean ASCII art (default): stroke characters (`_ - | / \`) along edges, a short density ramp for tone, symbols where there is fine detail, coloured ink on black; characters hold steady between video frames instead of flickering |
| `ascii` | Two-tone shape-matched ASCII: flat areas are clean colour, glyphs only where there is edge detail |
| `classic` | Shape-matched ASCII, colour ink on a dark ground |
| `sextant` | 2×3 two-colour mosaic per cell (sharpest) |
| `quadrant` | 2×2 two-colour mosaic |
| `halfblock` | Top/bottom half blocks |
| `braille` | 2×4 dot mosaic |

**Auto-exposure** (on by default, `E` toggles) lifts under-exposed video so dark scenes stay
readable: it measures each frame's 1st–99th percentile brightness, raises the highlights to white
with one gain for all channels (hues hold), eases in over ~0.2 s and resets on seeks. Frames
whose highlights already reach white are left alone, and the gain is capped at 3.5× so fades to
black don't become noise. `AE` on the player bar shows when it's on.

The ASCII modes match glyph *shape*, not just brightness, following the technique in Alex Harri's
["ASCII characters are not pixels"](https://alexharri.com/blog/ascii-rendering): each glyph is
rasterised from a real monospace font and described by its ink coverage in six staggered sampling
circles; image cells are sampled the same way, with directional and global contrast enhancement,
and the nearest glyph wins.

## Keys

| Key | Action |
|---|---|
| `/` `Ctrl-L` | search or open a URL |
| arrows / `hjkl`, `Enter` | move focus, open |
| `H` / `L` | back / forward |
| `Ctrl-T` `Ctrl-W` `J` `K` | new tab, close tab, next / previous tab |
| `space` `f` | pause, fullscreen |
| `,` `.` / `<` `>` / `0`–`9` | seek 10 s / 60 s / jump |
| `+` `-` `M` | volume up / down, mute |
| `C` `I` | show / hide live chat, send a chat message |
| `P` | save the focused / current video to a playlist |
| `V` | 3D audio visualiser instead of the video |
| `E` | auto-exposure on / off |
| `G` | pixel graphics (kitty / sixel) on / off |
| `n` `a` `x` | next video, autoplay toggle, close player |
| `w` `s` `c` `t` | watch later, subscribe, go to channel, open in new tab |
| composer | `Enter` post, `Alt-Enter` new line, `Esc` cancel (paste works) |
| `m` | cycle render mode |
| `?` `q` | help, quit |

Audio goes through PipeWire/PulseAudio (`pw-cat` or `pacat`) or ALSA (`aplay`), with ytui
applying the volume itself; without those tools ffmpeg plays audio directly. Pick the output
with `--audio`; it is off by default over SSH.

## License

MIT, see [LICENSE](LICENSE).
