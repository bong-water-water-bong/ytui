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
    println!("resolved in {:.2}s: {:?} ({} caption tracks, engine {})", t0.elapsed().as_secs_f64(), s.title, s.subs.len(), if opts.mpv { "mpv" } else { "ffmpeg" });
    for t in &s.subs {
        println!("  cc {} {}{}", t.lang, t.name, if t.auto { " [auto]" } else { "" });
    }
    let video = crate::api::VideoItem { title: s.title.clone(), ..Default::default() };
    let vol = std::env::var("YTUI_BENCH_VOL").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let mut p = Player::new(video, s, 0.0, opts, (cols, rows), vol, true);
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
