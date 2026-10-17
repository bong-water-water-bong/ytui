mod api;
mod app;
mod auth;
mod browser_login;
mod bench;
mod chat;
mod cookies;
mod demo;
mod diacritics;
mod engine3d;
mod gfx;
mod glyphs;
mod mpv;
mod out;
mod overlay;
mod page;
mod player;
mod proxy;
mod render;
mod scenes;
mod store;
mod sys;
mod textmode;
mod term;
mod thumbs;
mod uninstall;
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
  ytui --snapshot-ans SRC [-o FILE] [--at SECONDS] [--cols N --rows N]
                                save a frame as ANSI art (.ANS with SAUCE)
  ytui --convert-ans FILE [-o FILE]
                                re-encode ANSI art (normalise, or fix iCE)
  ytui --view-ans FILE
                                play ANSI art in the terminal, as-is
  ytui --import-subs FILE       import a Google Takeout subscriptions.csv
  ytui --login [--client FILE]  sign in with Google to like and comment
  ytui --logout                 sign out and revoke access
  ytui --help-login             how to create the Google OAuth client
  ytui --signin                 sign in for playback (when YouTube says
                                \"confirm you're not a bot\"): copy cookies from
                                your browser's cookie extension, ytui does the rest
  ytui --signout-cookies        forget those cookies
  ytui --help-cookies           more about that
  ytui --demo3d [SCENE]         3D ASCII engine showcase (logo, knot, terrain, visualizer)
  ytui --uninstall [--yes]      sign out and remove ytui, its settings and data

OPTIONS:
  --mode M       glow | art | ascii | classic | sextant | quadrant | halfblock |
                 braille | ansi | ansi9 | ansi-16 | ansi9-16 | ansiblink
  --audio A      auto | pulse | alsa | ffplay | none
                 (auto: pulse if available, ffplay on Windows, muted over ssh)
  --engine E     auto | mpv | ffmpeg: playback engine (auto: libmpv when found)
  --no-native    never use the native GPU video window (N toggles it; on Linux
                 and macOS it's a separate mpv window)
  --fullscreen   start the player fullscreen
  --fps N        max frame rate for the ffmpeg engine (default 60)
  --height N     max stream height to fetch (default: 720 in the terminal, so
                 60 fps streams are picked; the window's height for native video)
  --exposure X   on | off: auto-exposure for --snapshot (in the app, press E)
  --gfx X        auto | kitty | sixel | off: pixel graphics backend (default auto)
  --colors C     auto | 256 | true: colour depth (auto: 256 when tmux lacks RGB)
  --gfx-fps N    cap pixel video at N fps (default 12 over SSH, unlimited locally)
  --gfx-px N     longest edge of pixel images (default 960 over SSH, 1920 locally)
  --cookies FILE cookies.txt for yt-dlp (default: cookies.txt in the config
                 directory, if present; see --help-cookies)
  --cookies-from-browser B  read cookies from browser B (firefox works; Chrome
                 and Edge usually can't be read while running)
  --sixel-colors N  colours per sixel video frame, 2-255 (default 255; 64 is
                 ~40% faster to send but posterises soft shading)
  -o, --out FILE  output path for --snapshot-ans / --convert-ans
  --no-ice        write ANSI art with the background high bit as blink
                  (8 backgrounds) instead of iCE colours (16)
  --compact       write CR/LF-separated rows instead of absolute cursor
                  moves: smaller files, but clients that wrap at column 80
                  will band them
  --bits N        8 or 9: cell width in dots when writing ANSI art. 9 repeats
                  the last column of block characters, as real text mode does
  --palette       write 16-colour ANSI art instead of 24-bit (much smaller)

Press ? inside the app for all keys. State lives in ~/.local/share/ytui/.
ENV: YTUI_YTDLP (yt-dlp path), YTUI_FONT (monospace .ttf used for glyph shapes),
     YTUI_LIBMPV (path to libmpv), YTUI_ENGINE (as --engine)
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
    // is missing: find it, so mpv's window and the sign-in browser open.
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if let Some(n) = (0..4).map(|i| format!("wayland-{i}")).find(|n| std::path::Path::new(&dir).join(n).exists()) {
                std::env::set_var("WAYLAND_DISPLAY", n);
            }
        }
    }
    // mpv's window renders with Vulkan; Mesa's RADV prints a warning on
    // newer AMD GPUs straight onto ytui's screen otherwise.
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("MESA_VK_IGNORE_CONFORMANCE_WARNING").is_none() {
        std::env::set_var("MESA_VK_IGNORE_CONFORMANCE_WARNING", "1");
    }
    let mut args = std::env::args().skip(1);
    let mut mode = None;
    let mut audio = Audio::auto();
    let mut fps = 60;
    let mut engine = std::env::var("YTUI_ENGINE").unwrap_or_else(|_| "auto".into());
    let mut native = true;
    let mut fullscreen = false;
    let mut max_h: Option<u32> = None;
    let mut snapshot: Option<String> = None;
    let mut snapshot_ans: Option<String> = None;
    let mut convert_ans: Option<String> = None;
    let mut view_ans: Option<String> = None;
    let mut out_path: Option<String> = None;
    let mut ans_ice = true;
    let mut ans_compact = false;
    let mut ans_bits: usize = 8;
    let mut ans_truecolor = true;
    let mut bench: Option<String> = None;
    let mut bench_render: Option<String> = None;
    let mut secs = 8.0;
    let mut import: Option<String> = None;
    let mut login = false;
    let mut demo: Option<String> = None;
    let mut client: Option<String> = None;
    let mut at = 0.0;
    let mut exposure = true;
    let mut gfx_mode = std::env::var("YTUI_GFX").unwrap_or_else(|_| "auto".into());
    let mut gfx_fps: Option<f32> = None;
    let mut gfx_px: Option<usize> = None;
    let mut sixel_colors: Option<usize> = None;
    let mut colors = std::env::var("YTUI_COLORS").unwrap_or_else(|_| "auto".into());
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
            "--mode" => {
                let v = val("--mode")?;
                mode = Some(Mode::parse(&v).ok_or_else(|| anyhow::anyhow!("unknown mode {v}"))?);
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
            "--engine" => engine = val("--engine")?,
            "--no-native" => native = false,
            "--fullscreen" => fullscreen = true,
            "--height" => max_h = Some(val("--height")?.parse()?),
            "--snapshot" => snapshot = Some(val("--snapshot")?),
            "--snapshot-ans" => snapshot_ans = Some(val("--snapshot-ans")?),
            "--convert-ans" => convert_ans = Some(val("--convert-ans")?),
            "--view-ans" => view_ans = Some(val("--view-ans")?),
            "-o" | "--out" => out_path = Some(val("--out")?),
            "--no-ice" => ans_ice = false,
            "--compact" => ans_compact = true,
            "--truecolor" | "--24bit" => ans_truecolor = true,
            "--palette" | "--no-truecolor" => ans_truecolor = false,
            "--bits" => {
                ans_bits = val("--bits")?.parse()?;
                if ans_bits != 8 && ans_bits != 9 {
                    bail!("--bits must be 8 or 9");
                }
            }
            "--bench" => bench = Some(val("--bench")?),
            "--bench-render" => {
                let src = val("--bench-render")?;
                bench_render = Some(src);
            }
            "--probe-term" => return overlay::probe(&val("--probe-term")?),
            "--seconds" => secs = val("--seconds")?.parse()?,
            "--import-subs" => import = Some(val("--import-subs")?),
            "--login" => login = true,
            "--demo3d" => demo = Some(args.next().unwrap_or_default()),
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
            "--gfx" => gfx_mode = val("--gfx")?,
            "--colors" => colors = val("--colors")?,
            "--gfx-fps" => gfx_fps = Some(val("--gfx-fps")?.parse()?),
            "--gfx-px" => gfx_px = Some(val("--gfx-px")?.parse()?),
            "--sixel-colors" => sixel_colors = Some(val("--sixel-colors")?.parse()?),
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

    if let Some(src) = bench_render {
        return bench::render(&src, if at > 0.0 { at } else { 60.0 }, scols.unwrap_or(184), srows.unwrap_or(39), max_h.unwrap_or(480));
    }

    if let Some(src) = bench {
        let use_mpv = engine != "ffmpeg" && mpv::available();
        return bench::run(&src, secs, Opts { audio, fps, mpv: use_mpv, wid: None }, max_h.unwrap_or(720), renderer, scols.unwrap_or(120), srows.unwrap_or(40));
    }

    if let Some(path) = view_ans {
        return view_ans_file(&path, ans_ice);
    }

    // ANSI art output. Both paths return, since neither opens the UI.
    if convert_ans.is_some() || snapshot_ans.is_some() {
        let opts = AnsOpts {
            out: out_path.clone(),
            ice: if ans_ice { textmode::Ice::On } else { textmode::Ice::Off },
            ice_forced: !ans_ice,
            style: if ans_compact { textmode::Style::Compact } else { textmode::Style::Portable },
            bits: ans_bits,
            truecolor: ans_truecolor,
        };
        if let Some(path) = convert_ans {
            return write_ans_from_file(&path, &opts);
        }
        let src = snapshot_ans.expect("checked above");
        return write_ans_from_video(&src, at, &opts, exposure, scols, srows, max_h.unwrap_or(480));
    }

    if let Some(src) = snapshot {
        let (tc, tr) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((100, 30));
        let (input, aspect) = if std::path::Path::new(&src).exists() {
            (src.clone(), 16.0 / 9.0)
        } else {
            let s = yt::resolve(&src, max_h.unwrap_or(480), false)?;
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
                if let Some(c) = sixel_colors {
                    g.video_colors = c.clamp(2, 255);
                }
                g
            })
        }
    };
    let p256 = match colors.as_str() {
        "256" => true,
        "true" | "truecolor" | "24bit" => false,
        _ => gfx::tmux_lacks_rgb(),
    };
    render::PALETTE_256.store(p256, std::sync::atomic::Ordering::Relaxed);
    if p256 && colors == "auto" {
        hints.insert(0, "tmux limits colours to 256: add  set -as terminal-features ',xterm*:RGB'  to ~/.tmux.conf and reattach".into());
    }
    let use_mpv = match engine.as_str() {
        "ffmpeg" => false,
        "mpv" if !mpv::available() => bail!("--engine mpv: libmpv not found (install it or set YTUI_LIBMPV)"),
        _ => mpv::available(),
    };
    if !use_mpv && engine == "auto" {
        hints.insert(0, "libmpv not found: playing through ffmpeg (no captions); see README".into());
    }
    // The mosaic modes emit Unicode that a monospace font is not obliged to
    // have. Every font path we probe lacks braille and sextant, so say so
    // rather than let the user pick a mode that draws nothing but boxes.
    {
        let cov = glyphs::Glyphs::coverage();
        let missing: Vec<&str> = [
            ("sextant", cov.sextant),
            ("braille", cov.braille),
        ]
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(name, _)| *name)
        .collect();
        if !missing.is_empty() {
            hints.insert(
                0,
                format!(
                    "your font has no {} glyphs: set YTUI_FONT to a font that does",
                    missing.join(" or ")
                ),
            );
        }
    }
    let (mut app, rx) = app::App::new(renderer, Opts { audio, fps, mpv: use_mpv, wid: None }, max_h, mode.is_some());
    #[cfg(not(windows))]
    {
        let desktop = cfg!(target_os = "macos") || std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
        app.set_mpv_window(use_mpv && native && desktop && std::env::var("SSH_CONNECTION").is_err());
    }
    #[cfg(windows)]
    if use_mpv && native && std::env::var("SSH_CONNECTION").is_err() {
        if let Some(o) = overlay::Overlay::new() {
            app.set_overlay(o);
        }
    }
    app.set_gfx(gfx);
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
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste, crossterm::event::EnableFocusChange);
    let (out, buf) = out::Out::new();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(buf))?;
    app.set_fullscreen(fullscreen);
    if let Some(q) = start {
        app.open_query(&q);
    }
    let res = app.run(&mut terminal, &out, rx);
    out.drain();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture, crossterm::event::DisableBracketedPaste, crossterm::event::DisableFocusChange);
    ratatui::restore();
    res
}

/// How to write ANSI art, from the command line.
struct AnsOpts {
    out: Option<String>,
    ice: textmode::Ice,
    /// Did the user actually pass --no-ice, or is `ice` just the default?
    ice_forced: bool,
    style: textmode::Style,
    bits: usize,
    /// Write 24-bit colour into the file as PabloDraw sequences.
    truecolor: bool,
}

impl AnsOpts {
    fn write_opts(&self) -> textmode::WriteOpts {
        textmode::WriteOpts {
            style: self.style,
            ice: self.ice,
            sauce: true,
            bits: self.bits,
            truecolor: self.truecolor,
        }
    }
}

/// Write bytes to `out`, or to stdout when there is no path.
fn emit(bytes: &[u8], out: &Option<String>) -> Result<()> {
    match out {
        Some(p) => {
            std::fs::write(p, bytes)?;
            eprintln!("wrote {p} ({} bytes)", bytes.len());
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(bytes)?;
        }
    }
    Ok(())
}

/// Re-encode an existing `.ANS`: parse it, then write it back out.
///
/// Useful for normalising a file (absolute row placement, a fresh SAUCE) and
/// for re-reading art whose iCE bit the file got wrong, with `--no-ice`.
fn write_ans_from_file(path: &str, opts: &AnsOpts) -> Result<()> {
    let data = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let mut art = textmode::read_ansi(
        &data,
        textmode::ReadOpts { ice: opts.ice, width: None, use_sauce: true },
    );
    // Re-encode with the file's own iCE answer unless the caller insisted.
    // Otherwise `--convert-ans` would silently flip a file's background
    // interpretation, which is exactly what it is often used to check.
    let mut wo = opts.write_opts();
    if let (false, Some(s)) = (opts.ice_forced, art.sauce.as_ref()) {
        wo.ice = if s.ice() { textmode::Ice::On } else { textmode::Ice::Off };
    }
    if art.sauce.is_none() {
        // Give the output a record even if the input had none, so the result
        // is a well-formed art file.
        art.sauce = Some(textmode::Sauce {
            title: std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..Default::default()
        });
    }
    let bytes = textmode::write_ansi(&art, wo);
    if let Some(s) = &art.sauce {
        eprintln!(
            "{}x{} · {} · iCE {} · font {}",
            art.cols,
            art.rows,
            if s.title.is_empty() { "(untitled)" } else { &s.title },
            if s.ice() { "on" } else { "off" },
            s.font
        );
    }
    emit(&bytes, &opts.out)
}

/// Grab one frame of a video and save it as ANSI art.
fn write_ans_from_video(
    src: &str,
    at: f64,
    opts: &AnsOpts,
    exposure: bool,
    scols: Option<usize>,
    srows: Option<usize>,
    max_h: u32,
) -> Result<()> {
    let (_tc, _tr) = crossterm::terminal::size()
        .map(|(c, r)| (c as usize, r as usize))
        .unwrap_or((textmode::SCREEN_COLS, textmode::SCREEN_ROWS + 1));
    let (input, aspect) = if std::path::Path::new(src).exists() {
        (src.to_string(), 16.0 / 9.0)
    } else {
        let s = yt::resolve(src, max_h, false)?;
        (s.video_url, s.aspect)
    };
    // Art was drawn to 80x25, and cells are 8x16 dots, so the picture is
    // 640x400 unless the caller says otherwise.
    let cols = scols.unwrap_or(textmode::SCREEN_COLS);
    let rows = srows.unwrap_or(textmode::SCREEN_ROWS);
    let mut px = player::grab_frame(&input, at, cols * 8, rows * textmode::GLYPH_H)
        .ok_or_else(|| anyhow::anyhow!("could not decode a frame"))?;
    let mut ae = render::Exposure::new(exposure);
    ae.measure(&px, 1.0);
    ae.apply(&mut px);

    let art = if opts.truecolor {
        textmode::from_image_rgb(&px, cols * 8, rows * textmode::GLYPH_H, cols, rows, opts.bits)
    } else {
        textmode::from_image(&px, cols * 8, rows * textmode::GLYPH_H, cols, rows, opts.ice, textmode::Rgb::Vga, opts.bits)
    };
    let bytes = textmode::write_ansi(&art, opts.write_opts());
    eprintln!(
        "{}x{} cells, {} bytes, {}",
        art.cols,
        art.rows,
        bytes.len(),
        if opts.truecolor {
            "24-bit colour".to_string()
        } else {
            format!("16 colours, iCE {}", if opts.ice == textmode::Ice::On { "on" } else { "off" })
        }
    );
    let _ = aspect;
    emit(&bytes, &opts.out)
}

/// Show an ANSI art file on the terminal, exactly as its author drew it.
///
/// This is the "just look at it" path: the file is emitted with its own
/// attributes preserved, so the terminal does the rendering and what you see
/// is what the art says, not our interpretation of it.
fn view_ans_file(path: &str, ice_flag: bool) -> Result<()> {
    let data = std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let art = textmode::read_ansi(
        &data,
        textmode::ReadOpts {
            ice: if ice_flag { textmode::Ice::On } else { textmode::Ice::Off },
            width: None,
            use_sauce: true,
        },
    );
    if let Some(s) = &art.sauce {
        eprintln!(
            "{}{}{} · {}x{} · iCE {}",
            if s.title.is_empty() { "(untitled)".to_string() } else { s.title.clone() },
            if s.author.is_empty() { String::new() } else { format!(" by {}", s.author) },
            if s.group.is_empty() { String::new() } else { format!(" of {}", s.group) },
            art.cols,
            art.rows,
            if s.ice() { "on" } else { "off" },
        );
    }
    // Re-emit through the encoder so what appears is normalised (absolute row
    // placement, no reliance on the terminal's own wrapping) rather than the
    // original byte soup, which may assume a specific client.
    let bytes = textmode::write_ansi(
        &art,
        textmode::WriteOpts {
            style: textmode::Style::Portable,
            ice: if sweet_ice(&art, ice_flag) { textmode::Ice::On } else { textmode::Ice::Off },
            sauce: false,
            bits: 8,
            truecolor: false,
        },
    );
    emit(&bytes, &None)
}

/// Which iCE answer to use for a file: its own SAUCE flag if it has one,
/// otherwise the caller's.
fn sweet_ice(art: &textmode::Art, fallback: bool) -> bool {
    match &art.sauce {
        Some(s) if s.datatype == 1 => s.ice(),
        _ => fallback,
    }
}
