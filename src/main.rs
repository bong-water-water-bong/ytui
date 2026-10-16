mod api;
mod app;
mod auth;
mod chat;
mod demo;
mod diacritics;
mod engine3d;
mod gfx;
mod glyphs;
mod page;
mod player;
mod render;
mod scenes;
mod store;
mod term;
mod thumbs;
mod yt;

use anyhow::{bail, Result};
use glyphs::{CH, CW};
use page::Kind;
use player::{Audio, Opts};
use render::{Img, Mode, Renderer};

const HELP: &str = "\
ytui - YouTube in the terminal: a full browser-style client with videos
rendered as colour ASCII / Unicode mosaics

USAGE:
  ytui                          open Home
  ytui QUERY...                 open a search
  ytui URL                      open a video, playlist or channel URL
  ytui --snapshot SRC [--at SECONDS] [--cols N --rows N]
                                print one rendered frame (SRC = URL or file)
  ytui --import-subs FILE       import a Google Takeout subscriptions.csv
  ytui --login [--client FILE]  sign in with Google to like and comment
  ytui --logout                 sign out and revoke access
  ytui --help-login             how to create the Google OAuth client
  ytui --demo3d [SCENE]         3D ASCII engine showcase (logo, knot, terrain, visualizer)

OPTIONS:
  --mode M       art | ascii | classic | sextant | quadrant | halfblock | braille
  --audio A      auto | pulse | alsa | none   (auto: pulse if available, muted over ssh)
  --fps N        max render frame rate (default 30)
  --height N     max stream height to fetch (default 480)
  --exposure X   on | off: auto-exposure for --snapshot (in the app, press E)
  --gfx X        auto | kitty | sixel | off: pixel graphics backend (default auto)
  --gfx-fps N    cap pixel video at N fps (default 12 over SSH, unlimited locally)
  --gfx-px N     longest edge of pixel images (default 640 over SSH, 1280 locally)

Press ? inside the app for all keys. State lives in ~/.local/share/ytui/.
ENV: YTUI_YTDLP (yt-dlp path), YTUI_FONT (monospace .ttf used for glyph shapes)
";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut mode = None;
    let mut audio = Audio::auto();
    let mut fps = 30;
    let mut max_h = 480;
    let mut snapshot: Option<String> = None;
    let mut import: Option<String> = None;
    let mut login = false;
    let mut demo: Option<String> = None;
    let mut client: Option<String> = None;
    let mut at = 0.0;
    let mut exposure = true;
    let mut gfx_mode = std::env::var("YTUI_GFX").unwrap_or_else(|_| "auto".into());
    let mut gfx_fps: Option<f32> = None;
    let mut gfx_px: Option<usize> = None;
    let (mut scols, mut srows) = (None, None);
    let mut query = Vec::new();
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "--mode" => {
                let v = val("--mode")?;
                mode = Some(Mode::parse(&v).ok_or_else(|| anyhow::anyhow!("unknown mode {v}"))?);
            }
            "--audio" => {
                audio = match val("--audio")?.as_str() {
                    "auto" => Audio::auto(),
                    "pulse" => Audio::Pulse,
                    "alsa" => Audio::Alsa,
                    "none" | "off" => Audio::None,
                    v => bail!("unknown audio output {v}"),
                }
            }
            "--fps" => fps = val("--fps")?.parse()?,
            "--height" => max_h = val("--height")?.parse()?,
            "--snapshot" => snapshot = Some(val("--snapshot")?),
            "--import-subs" => import = Some(val("--import-subs")?),
            "--login" => login = true,
            "--demo3d" => demo = Some(args.next().unwrap_or_default()),
            "--client" => client = Some(val("--client")?),
            "--logout" => return auth::logout(),
            "--help-login" => {
                print!("{}", auth::SETUP_HELP);
                return Ok(());
            }
            "--at" => at = val("--at")?.parse()?,
            "--gfx" => gfx_mode = val("--gfx")?,
            "--gfx-fps" => gfx_fps = Some(val("--gfx-fps")?.parse()?),
            "--gfx-px" => gfx_px = Some(val("--gfx-px")?.parse()?),
            "--exposure" => exposure = !matches!(val("--exposure")?.as_str(), "off" | "no" | "0" | "false"),
            "--cols" => scols = Some(val("--cols")?.parse()?),
            "--rows" => srows = Some(val("--rows")?.parse()?),
            _ => query.push(a),
        }
    }

    if login {
        return auth::login(client.as_deref());
    }

    if let Some(f) = import {
        let mut st = store::Store::load();
        let n = st.import_csv(&std::fs::read_to_string(&f)?);
        st.save();
        println!("imported {n} subscriptions ({} total)", st.subs.len());
        return Ok(());
    }

    let mut renderer = Renderer::new(mode.unwrap_or(Mode::Art));

    if let (Some(d), Ok(path)) = (&demo, std::env::var("YTUI_3D_DUMP")) {
        let kind = scenes::Kind::parse(d).unwrap_or(scenes::Kind::Logo);
        let t = std::env::var("YTUI_3D_T").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        return demo::dump(kind, t, 1080, 588, &path);
    }
    if let Some(d) = demo {
        let kind = scenes::Kind::parse(&d).unwrap_or(scenes::Kind::Logo);
        if mode.is_none() {
            renderer.mode = Mode::Classic;
        }
        return demo::run(kind, renderer);
    }

    if let Some(src) = snapshot {
        let (tc, tr) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((100, 30));
        let (input, aspect) = if std::path::Path::new(&src).exists() {
            (src.clone(), 16.0 / 9.0)
        } else {
            let s = yt::resolve(&src, max_h)?;
            (s.video_url, s.aspect)
        };
        let (w, h, _, _) = player::fit(aspect, scols.unwrap_or(tc), srows.unwrap_or(tr.saturating_sub(1)));
        let mut px = player::grab_frame(&input, at, w * CW, h * CH).ok_or_else(|| anyhow::anyhow!("could not decode a frame"))?;
        let mut ae = render::Exposure::new(exposure);
        ae.measure(&px, 1.0);
        ae.apply(&mut px);
        if exposure {
            let (lo, gain) = ae.params();
            eprintln!("auto-exposure: offset {lo:.3}, gain {gain:.2}");
        }
        let mut cells = Vec::new();
        renderer.render(&Img { w: w * CW, h: h * CH, px: &px }, w, h, &mut cells);
        print!("{}", term::to_ansi(&cells, w, h));
        return Ok(());
    }

    player::clean_stale_fifos();
    let mut hints: Vec<String> = Vec::new();
    // Ask the terminal about pixel graphics before the UI takes it over.
    let gfx = match gfx_mode.as_str() {
        "off" | "none" | "no" => None,
        m => {
            let caps = gfx::detect();
            hints = caps.why_not().into_iter().collect();
            if caps.tmux_256 {
                hints.push("tmux is cutting colours to 256: add  set -as terminal-features ',xterm-256color:RGB'  to ~/.tmux.conf".into());
            }
            let proto = match m {
                "kitty" => Some(gfx::Proto::Kitty),
                "sixel" => Some(gfx::Proto::Sixel),
                _ => caps.choose(),
            };
            proto.map(|p| {
                let mut g = gfx::Gfx::new(p, &caps);
                if let Some(f) = gfx_fps {
                    g.max_fps = f;
                }
                if let Some(px) = gfx_px {
                    g.max_px = px.max(64);
                }
                g
            })
        }
    };
    let (mut app, rx) = app::App::new(renderer, Opts { audio, fps }, max_h, mode.is_some());
    app.set_gfx(gfx);
    if let Some(h) = hints.last() {
        app.hint(h.clone());
    }
    let chip = app.default_chip();
    app.open_tab(Kind::Home { chip, feed: api::Feed::default() });
    let q = query.join(" ");
    let start = (!q.trim().is_empty()).then_some(q);

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste);
    if let Some(q) = start {
        app.open_query(&q);
    }
    let res = app.run(&mut terminal, rx);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture, crossterm::event::DisableBracketedPaste);
    ratatui::restore();
    res
}
