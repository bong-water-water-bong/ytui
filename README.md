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
- **Miniplayer** when you scroll or browse away, and **fullscreen** (`f`)
- **Channel pages** (banner, avatar, Home/Videos/Shorts/Live/Playlists tabs) and **playlists**
- **Subscriptions, history (with resume positions) and watch later**, stored locally in
  `~/.local/share/ytui/`
- **Mouse**: click anything, wheel to scroll, click the progress bar to seek

- **Sign in** (optional) to like / dislike videos, comment and reply, using Google's
  official OAuth and the YouTube Data API (see below)

Browsing uses YouTube's own web JSON API directly (no API key, no account); `yt-dlp` is only used
to resolve stream URLs. Press `?` in the app for every key.

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

While the OAuth app is in *Testing*, Google expires the sign-in after 7 days (run `ytui --login`
again, or publish the app). The free quota is 10,000 units/day; a like or comment costs 50.
`ytui --logout` revokes access. `ytui --help-login` prints these steps.

## Render modes (`m` cycles)

| Mode | Description |
|---|---|
| `ascii` | Two-tone shape-matched ASCII: flat areas are clean colour, glyphs only where there is edge detail |
| `classic` | Shape-matched ASCII, colour ink on a dark ground |
| `sextant` | 2×3 two-colour mosaic per cell (sharpest) |
| `quadrant` | 2×2 two-colour mosaic |
| `halfblock` | Top/bottom half blocks |
| `braille` | 2×4 dot mosaic |

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
| `C` | show / hide live chat |
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
