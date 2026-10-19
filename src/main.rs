// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

mod api;
mod app;
mod auth;
mod browser_login;
mod bench;
mod chat;
mod cookies;
mod music;
mod out;
mod page;
mod player;
mod proxy;
mod render;
mod store;
mod sys;
mod term;
mod thumbs;
mod uninstall;
mod yt;

use anyhow::{bail, Result};
use render::{CH, CW};
use page::Kind;
use player::{Audio, Opts};
use render::{Img, Renderer};

const HELP: &str = "\
ytui - YouTube in the terminal: a full browser-style client with videos
drawn as 24-bit colour half blocks

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
  ytui --signin                 sign in for playback (when YouTube says
                                \"confirm you're not a bot\"): copy cookies from
                                your browser's cookie extension, ytui does the rest
  ytui --signout-cookies        forget those cookies
  ytui --help-cookies           more about that
  ytui --uninstall [--yes]      sign out and remove ytui, its settings and data

OPTIONS:
  --audio A      auto | pulse | alsa | ffplay | none
                 (auto: pulse if available, ffplay on Windows, muted over ssh)
  --fullscreen   start the player fullscreen
  --fps N        max frame rate (default 60)
  --hwdec X      auto | off: decode video on the GPU (VAAPI, VideoToolbox,
                 D3D11VA; default auto, software if the GPU can't)
  --height N     max stream height to fetch (default 720, the lowest height
                 YouTube serves at 60 fps)
  --exposure X   on | off: auto-exposure for --snapshot (in the app, press E)
  --cookies FILE cookies.txt for yt-dlp (default: cookies.txt in the config
                 directory, if present; see --help-cookies)
  --cookies-from-browser B  read cookies from browser B (firefox works; Chrome
                 and Edge usually can't be read while running)

Press ? inside the app for all keys. State lives in ~/.local/share/ytui/.
ENV: YTUI_YTDLP (yt-dlp path)
";

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--color-test") {
        if std::env::var_os("YTUI_NO_INIT").is_none() {
            sys::init();
        }
        return bench::color_test(&std::env::args().skip(2).collect::<Vec<_>>());
    }
    sys::init();
    // In a tmux started outside the desktop, the desktop's Wayland display
    // is missing: find it, so the sign-in browser opens.
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if let Some(n) = (0..4).map(|i| format!("wayland-{i}")).find(|n| std::path::Path::new(&dir).join(n).exists()) {
                std::env::set_var("WAYLAND_DISPLAY", n);
            }
        }
    }
    let mut args = std::env::args().skip(1);
    let mut audio = Audio::auto();
    let mut fps = 60;
    let mut fullscreen = false;
    let mut max_h: Option<u32> = None;
    let mut snapshot: Option<String> = None;
    let mut bench: Option<String> = None;
    let mut bench_render: Option<String> = None;
    let mut secs = 8.0;
    let mut import: Option<String> = None;
    let mut login = false;
    let mut client: Option<String> = None;
    let mut at = 0.0;
    let mut exposure = true;
    let (mut scols, mut srows) = (None, None);
    let mut query = Vec::new();
    let mut cookies: Option<String> = std::env::var("YTUI_COOKIES").ok();
    let mut cookies_browser: Option<String> = None;
    let mut signin = false;
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "--audio" => {
                audio = match val("--audio")?.as_str() {
                    "auto" => Audio::auto(),
                    "pulse" => Audio::Pulse,
                    "alsa" => Audio::Alsa,
                    "ffplay" => Audio::Ffplay,
                    "none" | "off" => Audio::None,
                    v => bail!("unknown audio output {v}"),
                }
            }
            "--fps" => fps = val("--fps")?.parse()?,
            "--hwdec" => match val("--hwdec")?.as_str() {
                "auto" | "on" => {}
                "off" | "no" => player::hwdec_off(),
                v => bail!("unknown --hwdec value {v} (auto or off)"),
            },
            "--fullscreen" => fullscreen = true,
            "--height" => max_h = Some(val("--height")?.parse()?),
            "--snapshot" => snapshot = Some(val("--snapshot")?),
            "--bench" => bench = Some(val("--bench")?),
            "--music-dump" => return music::dump(&val("--music-dump")?),
            "--bench-render" => {
                let src = val("--bench-render")?;
                bench_render = Some(src);
            }
            "--seconds" => secs = val("--seconds")?.parse()?,
            "--import-subs" => import = Some(val("--import-subs")?),
            "--login" => login = true,
            "--client" => client = Some(val("--client")?),
            "--logout" => return auth::logout(),
            "--uninstall" => return uninstall::run(args.any(|a| a == "--yes" || a == "-y")),
            "--signin" => signin = true,
            "--signin-probe" => return browser_login::probe(),
            "--signin-titles" => return browser_login::log_titles(),
            "--signin-hold" => return browser_login::hold_test(),
            "--whoami" => {
                let api = api::Api::new();
                println!("signed in: {}", api.signed_in());
                println!("account: {:?}", api.account_name());
                println!("home: {:?}", api.home().map(|f| f.videos().iter().take(3).map(|v| v.title.clone()).collect::<Vec<_>>()));
                println!("subscriptions feed: {:?}", api.subscriptions_feed().map(|f| f.videos().len()));
                match api.subscribed_channels() {
                    Ok(c) => println!("subscribed channels: {} (first: {:?})", c.len(), c.iter().take(3).map(|c| c.name.clone()).collect::<Vec<_>>()),
                    Err(e) => println!("subscribed channels: error {e}"),
                }
                match api.my_playlists() {
                    Ok(l) => println!("playlists: {} (first: {:?})", l.len(), l.iter().take(3).map(|p| (p.title.clone(), p.count.clone())).collect::<Vec<_>>()),
                    Err(e) => println!("playlists: error {e}"),
                }
                return Ok(());
            }
            "--signout-cookies" => {
                cookies::remove();
                println!("removed {}", cookies::path().display());
                return Ok(());
            }
            "--help-cookies" => {
                print!("{}", yt::COOKIES_HELP.replace("COOKIES_PATH", &yt::cookies_path().display().to_string()));
                return Ok(());
            }
            "--cookies" => cookies = Some(val("--cookies")?),
            "--cookies-from-browser" => cookies_browser = Some(val("--cookies-from-browser")?),
            "--help-login" => {
                print!("{}", auth::SETUP_HELP);
                return Ok(());
            }
            "--at" => at = val("--at")?.parse()?,
            "--exposure" => exposure = !matches!(val("--exposure")?.as_str(), "off" | "no" | "0" | "false"),
            "--cols" => scols = Some(val("--cols")?.parse()?),
            "--rows" => srows = Some(val("--rows")?.parse()?),
            _ => query.push(a),
        }
    }

    yt::set_cookies(cookies, cookies_browser);
    if signin {
        return cookies::signin_cli();
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

    let renderer = Renderer::new();

    if let Some(src) = bench_render {
        return bench::render(&src, if at > 0.0 { at } else { 60.0 }, scols.unwrap_or(184), srows.unwrap_or(39), max_h.unwrap_or(480));
    }

    if let Some(src) = bench {
        return bench::run(&src, secs, Opts { audio, fps }, max_h.unwrap_or(720), renderer, scols.unwrap_or(120), srows.unwrap_or(40));
    }

    if let Some(src) = snapshot {
        let (tc, tr) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((100, 30));
        let (input, aspect) = if std::path::Path::new(&src).exists() {
            (src.clone(), 16.0 / 9.0)
        } else {
            let s = yt::resolve(&src, max_h.unwrap_or(480))?;
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
        Renderer::cells(&Img { w: w * CW, h: h * CH, px: &px }, w, h, &mut cells);
        print!("{}", term::to_ansi(&cells, w, h));
        return Ok(());
    }

    player::clean_stale_fifos();
    let mut hints: Vec<String> = Vec::new();
    if tmux_lacks_rgb() {
        hints.push("tmux limits colours to 256: add  set -as terminal-features ',xterm*:RGB'  to ~/.tmux.conf and reattach".into());
    }
    let (mut app, rx) = app::App::new(renderer, Opts { audio, fps }, max_h);
    if let Some(h) = hints.last() {
        app.hint(h.clone());
    }
    let chip = app.default_chip();
    app.open_tab(Kind::Home { chip, feed: api::Feed::default() });
    let q = query.join(" ");
    let start = (!q.trim().is_empty()).then_some(q);

    // NO_COLOR is for output that merely decorates; here colour is the
    // picture itself, so keep it on.
    crossterm::style::force_color_output(true);
    // Raw mode, alternate screen and the panic hook; drawing goes through
    // our own output thread instead of straight to stdout.
    drop(ratatui::init());
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste);
    let (out, buf) = out::Out::new();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(buf))?;
    app.set_fullscreen(fullscreen);
    if let Some(q) = start {
        app.open_query(&q);
    }
    let res = app.run(&mut terminal, &out, rx);
    out.drain();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture, crossterm::event::DisableBracketedPaste);
    ratatui::restore();
    res
}

/// tmux drawing to a terminal it doesn't know does 24-bit colour: it then
/// rounds every colour to the 256-colour palette.
fn tmux_lacks_rgb() -> bool {
    std::env::var_os("TMUX").is_some()
        && std::process::Command::new("tmux")
            .args(["display-message", "-p", "#{client_termfeatures}"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .is_some_and(|f| !f.is_empty() && !f.split(',').any(|x| x == "RGB"))
}
