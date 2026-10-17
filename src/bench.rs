//! Headless playback check (`--bench SRC`): plays through the real player
//! for a while without a terminal UI and reports frame rate, render cost,
//! seek / pause behaviour and captions.

use crate::player::{Opts, Player};
use crate::render::Renderer;
use anyhow::Result;
use std::time::{Duration, Instant};

pub fn run(src: &str, secs: f64, opts: Opts, max_h: u32, mut r: Renderer, cols: usize, rows: usize) -> Result<()> {
    let t0 = Instant::now();
    let s = crate::yt::resolve(src, max_h)?;
    println!("resolved in {:.2}s: {:?} ({} caption tracks)", t0.elapsed().as_secs_f64(), s.title, s.subs.len());
    for t in &s.subs {
        println!("  cc {} {}{}", t.lang, t.name, if t.auto { " [auto]" } else { "" });
    }
    let video = crate::api::VideoItem { title: s.title.clone(), ..Default::default() };
    let vol = std::env::var("YTUI_BENCH_VOL").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let mut p = Player::new(video, s, 0.0, opts, (cols, rows), vol, true);
    if let Some((a, b)) = std::env::var("YTUI_BENCH_PX").ok().and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))) {
        p.set_px_cell((a, b));
    }
    if std::env::var("YTUI_BENCH_CC").is_ok() {
        p.set_caption_pref(Some(std::env::var("YTUI_BENCH_CC").unwrap()));
    }
    let start = Instant::now();
    let mut first: Option<f64> = None;
    let (mut frames, mut render_ms) = (0u32, 0f64);
    let mut last_cc = String::new();
    let mut step = 0;
    let mut seek_at: Option<(Instant, u64)> = None;
    while start.elapsed().as_secs_f64() < secs {
        if p.tick() {
            let rt = Instant::now();
            if p.cells(&mut r, cols, rows).is_some() {
                frames += 1;
                render_ms += rt.elapsed().as_secs_f64() * 1000.0;
                if first.is_none() {
                    first = Some(start.elapsed().as_secs_f64());
                    println!("first frame after {:.2}s", first.unwrap());
                }
                if let Some((t, n)) = seek_at {
                    if frames as u64 > n + 1 {
                        println!("  seek showed a new frame after {:.0} ms (pos {:.1})", t.elapsed().as_secs_f64() * 1000.0, p.pos);
                        seek_at = None;
                    }
                }
            }
        }
        let cc = p.caption_text().replace('\n', " / ");
        if cc != last_cc {
            if !cc.is_empty() {
                println!("  [{:6.2}] cc: {cc}", p.pos);
            }
            last_cc = cc;
        }
        if p.ended || p.failed {
            println!("ended={} failed={} note={:?}", p.ended, p.failed, p.note);
            break;
        }
        let el = start.elapsed().as_secs_f64();
        if let Some(f) = first {
            // Script: play 3 s, seek +30 s, play 2 s, pause 1 s, resume.
            if step == 0 && el > f + 3.0 {
                println!("at {:.1}s pos {:.2}: seek +30", el, p.pos);
                seek_at = Some((Instant::now(), frames as u64));
                p.seek_by(30.0);
                step = 1;
            } else if step == 1 && el > f + 5.0 {
                let before = p.pos;
                p.toggle_pause();
                std::thread::sleep(Duration::from_millis(1000));
                p.tick();
                println!("paused 1s: pos {before:.2} -> {:.2} (should not move)", p.pos);
                p.toggle_pause();
                step = 2;
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let el = start.elapsed().as_secs_f64() - first.unwrap_or(0.0);
    println!(
        "{frames} frames in {el:.1}s = {:.1} fps shown, render {:.2} ms/frame ({}), pos {:.2}, note {:?}",
        frames as f64 / el.max(0.001),
        render_ms / frames.max(1) as f64,
        r.mode.name(),
        p.pos,
        p.note
    );
    Ok(())
}

/// Decode `n` consecutive frames of `src` at w x h (from `at` seconds).
fn frames(input: &str, at: f64, n: usize, w: usize, h: usize) -> Result<Vec<Vec<u8>>> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss", &format!("{at}"), "-i", &crate::proxy::wrap(&input), "-frames:v", &n.to_string(), "-vf"])
        .arg(format!("scale={w}:{h}:flags=area,format=rgb24"))
        .args(["-f", "rawvideo", "pipe:1"])
        .output()?;
    if out.stdout.len() < w * h * 3 {
        anyhow::bail!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout.chunks_exact(w * h * 3).map(|c| c.to_vec()).collect())
}

/// Bytes a ratatui/crossterm-style diff would write to go from `prev` to
/// `cur`: cursor moves where runs break, colour escapes where they change.
fn diff_bytes(prev: Option<&[crate::render::Cell]>, cur: &[crate::render::Cell], cols: usize) -> usize {
    let mut n = 0;
    let (mut last_fg, mut last_bg) = (None, None);
    let mut next_pos = usize::MAX;
    for (i, c) in cur.iter().enumerate() {
        if prev.is_some_and(|p| p[i] == *c) {
            continue;
        }
        if i != next_pos {
            n += format!("\x1b[{};{}H", i / cols + 1, i % cols + 1).len();
        }
        if last_fg != Some(c.fg) {
            n += format!("\x1b[38;2;{};{};{}m", c.fg[0], c.fg[1], c.fg[2]).len();
            last_fg = Some(c.fg);
        }
        if last_bg != Some(c.bg) {
            n += format!("\x1b[48;2;{};{};{}m", c.bg[0], c.bg[1], c.bg[2]).len();
            last_bg = Some(c.bg);
        }
        n += c.ch.len_utf8();
        next_pos = i + 1;
    }
    n
}

/// `--bench-render SRC`: renderer and encoder cost per mode at a terminal
/// size, over consecutive real video frames.
pub fn render(src: &str, at: f64, cols: usize, rows: usize, max_h: u32) -> Result<()> {
    use crate::glyphs::{CH, CW};
    use crate::render::{Img, Mode, Renderer};
    const N: usize = 60;
    let (w, h) = (cols * CW, rows * CH);
    let input = if std::path::Path::new(src).exists() { src.to_string() } else { crate::yt::resolve(src, max_h)?.video_url };
    let fr = frames(&input, at, N, w, h)?;
    println!("{} frames at {cols}x{rows} cells ({w}x{h} px)", fr.len());
    for mode in Mode::ALL {
        let mut r = Renderer::new(mode);
        r.temporal = true;
        let mut cells = Vec::new();
        let mut prev: Option<Vec<crate::render::Cell>> = None;
        let (mut ms, mut bytes) = (Vec::new(), 0usize);
        for f in &fr {
            let t = Instant::now();
            r.render(&Img { w, h, px: f }, cols, rows, &mut cells);
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            bytes += diff_bytes(prev.as_deref(), &cells, cols);
            prev = Some(cells.clone());
        }
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{:9} render median {:6.2} ms  p95 {:6.2} ms  | output {:6.1} KB/frame = {:5.2} MB/s at 30 fps",
            mode.name(),
            ms[ms.len() / 2],
            ms[ms.len() * 95 / 100],
            bytes as f64 / fr.len() as f64 / 1024.0,
            bytes as f64 / fr.len() as f64 * 30.0 / 1e6
        );
    }
    // Sixel at Windows Terminal's nominal 10x20 cell: palettes x aspect.
    use crate::gfx::{quantize, sixel_emit, Palette};
    for aspect in [1, 2] {
        let (sw, sh) = (cols * 10, rows * 20 / aspect);
        let sf = frames(&input, at, 20, sw, sh)?;
        let mut pals = vec![Palette::Fixed];
        for colors in [32, 64, 128, 255] {
            for dither in [0, 8, 16, 32] {
                pals.push(Palette::Adaptive { colors, dither });
            }
        }
        if aspect == 1 {
            pals = vec![Palette::Fixed, crate::gfx::VIDEO_PALETTE];
        }
        for pal in pals {
            let (mut ms, mut bytes, mut se, mut n) = (Vec::new(), 0usize, 0f64, 0usize);
            for (fi, f) in sf.iter().enumerate() {
                let mut out = Vec::new();
                let t = Instant::now();
                let (colors, idx) = quantize(f, sw, sh, pal);
                let tq = t.elapsed().as_secs_f64() * 1000.0;
                if fi == 0 && aspect == 2 && matches!(pal, Palette::Adaptive { colors: 255, dither: 8 }) {
                    let te = Instant::now();
                    let mut tmp = Vec::new();
                    sixel_emit(&colors, &idx, sw, sh, aspect, &mut tmp);
                    println!("  breakdown 255c d8 2:1: quantize {tq:.2} ms, emit {:.2} ms", te.elapsed().as_secs_f64() * 1000.0);
                }
                if let (0, Ok(dir)) = (fi, std::env::var("YTUI_BENCH_PNG")) {
                    // The quantised picture as shown (rows repeated for 2:1).
                    let mut img = image::RgbImage::new(sw as u32, (sh * aspect) as u32);
                    for (y, row) in idx.chunks_exact(sw).enumerate() {
                        for (x, &i) in row.iter().enumerate() {
                            for k in 0..aspect {
                                img.put_pixel(x as u32, (y * aspect + k) as u32, image::Rgb(colors[i as usize]));
                            }
                        }
                    }
                    let name = match pal { Palette::Fixed => "fixed".to_string(), Palette::Adaptive { colors, dither } => format!("{colors}c-d{dither}") };
                    let _ = img.save(format!("{dir}/q-{aspect}x-{name}.png"));
                    if pal == Palette::Fixed {
                        let src = image::RgbImage::from_raw(sw as u32, sh as u32, f.clone()).unwrap();
                        let _ = image::imageops::resize(&src, sw as u32, (sh * aspect) as u32, image::imageops::FilterType::Nearest).save(format!("{dir}/q-{aspect}x-source.png"));
                    }
                }
                sixel_emit(&colors, &idx, sw, sh, aspect, &mut out);
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
                bytes += out.len();
                for (p, &i) in f.chunks_exact(3).zip(&idx) {
                    let c = colors[i as usize];
                    for k in 0..3 {
                        se += (p[k] as f64 - c[k] as f64).powi(2);
                    }
                    n += 3;
                }
            }
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let psnr = 10.0 * (255f64 * 255.0 / (se / n as f64)).log10();
            println!(
                "sixel {aspect}:1 {:9} encode median {:6.2} ms ({sw}x{sh}) | {:6.1} KB/frame = {:5.1} MB/s at 60 fps | PSNR {psnr:5.2} dB",
                match pal { Palette::Fixed => "fixed".to_string(), Palette::Adaptive { colors, dither } => format!("{colors}c d{dither}") },
                ms[ms.len() / 2],
                bytes as f64 / sf.len() as f64 / 1024.0,
                bytes as f64 / sf.len() as f64 * 60.0 / 1e6
            );
        }
    }
    Ok(())
}

/// `--color-test [detect] [focus]`: an RGB gradient through ratatui,
/// optionally after ytui's startup steps, to bisect colour problems.
pub fn color_test(steps: &[String]) -> Result<()> {
    use ratatui::style::{Color, Style};
    let has = |s: &str| steps.iter().any(|x| x == s);
    if has("detect") {
        let _ = crate::gfx::detect();
    }
    if !has("nocolor") {
        crossterm::style::force_color_output(true);
    }
    let mut terminal = ratatui::init();
    if has("focus") {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste, crossterm::event::EnableFocusChange);
    }
    let label = format!("steps {steps:?} | {}", crate::sys::console_state());
    for _ in 0..40 {
        terminal.draw(|f| {
            let a = f.area();
            let buf = f.buffer_mut();
            for y in 0..a.height {
                for x in 0..a.width {
                    let r = (x as u32 * 255 / a.width.max(1) as u32) as u8;
                    let g = (y as u32 * 255 / a.height.max(1) as u32) as u8;
                    buf[(x, y)].set_char('▀').set_fg(Color::Rgb(r, g, 255 - r)).set_bg(Color::Rgb(g, r, 128));
                }
            }
            buf.set_string(1, 1, &label, Style::default().fg(Color::Rgb(255, 0, 0)).bg(Color::Rgb(0, 0, 0)));
        })?;
        std::thread::sleep(Duration::from_millis(100));
    }
    ratatui::restore();
    Ok(())
}
