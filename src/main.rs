mod api;
mod app;
mod chat;
mod glyphs;
mod page;
mod player;
mod render;
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

OPTIONS:
  --mode M       ascii | classic | sextant | quadrant | halfblock | braille
  --audio A      auto | pulse | alsa | none   (auto: pulse if available, muted over ssh)
  --fps N        max render frame rate (default 30)
  --height N     max stream height to fetch (default 480)

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
    let mut at = 0.0;
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
            "--at" => at = val("--at")?.parse()?,
            "--cols" => scols = Some(val("--cols")?.parse()?),
            "--rows" => srows = Some(val("--rows")?.parse()?),
            _ => query.push(a),
        }
    }

    if let Some(f) = import {
        let mut st = store::Store::load();
        let n = st.import_csv(&std::fs::read_to_string(&f)?);
        st.save();
        println!("imported {n} subscriptions ({} total)", st.subs.len());
        return Ok(());
    }

    let mut renderer = Renderer::new(mode.unwrap_or(Mode::Ascii));

    if let Some(src) = snapshot {
        let (tc, tr) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((100, 30));
        let (input, aspect) = if std::path::Path::new(&src).exists() {
            (src.clone(), 16.0 / 9.0)
        } else {
            let s = yt::resolve(&src, max_h)?;
            (s.video_url, s.aspect)
        };
        let (w, h, _, _) = player::fit(aspect, scols.unwrap_or(tc), srows.unwrap_or(tr.saturating_sub(1)));
        let px = player::grab_frame(&input, at, w * CW, h * CH).ok_or_else(|| anyhow::anyhow!("could not decode a frame"))?;
        let mut cells = Vec::new();
        renderer.render(&Img { w: w * CW, h: h * CH, px: &px }, w, h, &mut cells);
        print!("{}", term::to_ansi(&cells, w, h));
        return Ok(());
    }

    player::clean_stale_fifos();
    let (mut app, rx) = app::App::new(renderer, Opts { audio, fps }, max_h, mode.is_some());
    let chip = app.default_chip();
    app.open_tab(Kind::Home { chip, feed: api::Feed::default() });
    let q = query.join(" ");
    let start = (!q.trim().is_empty()).then_some(q);

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    if let Some(q) = start {
        app.open_query(&q);
    }
    let res = app.run(&mut terminal, rx);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    res
}
