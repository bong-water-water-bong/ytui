//! `ytui --demo3d`: the 3D engine on its own, full screen, with an fps
//! counter. The visualiser scene is fed by a synthesised beat so the audio
//! analyser runs too.

use crate::engine3d::Engine;
use crate::glyphs::{CH, CW};
use crate::render::{Img, Renderer};
use crate::scenes::{Analyzer, Kind, Orbit, Scenes};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::style::{Color, Style};
use std::time::{Duration, Instant};

/// A little synth loop: kick, a minor chord, and hi-hats.
fn synth(t0: f64, n: usize) -> Vec<f32> {
    let sr = 48000.0;
    (0..n)
        .map(|i| {
            let t = t0 + i as f64 / sr;
            let beat = t % 0.5;
            let kick = (std::f64::consts::TAU * (50.0 + 90.0 * (-beat * 30.0).exp()) * beat).sin() * (-beat * 9.0).exp();
            let bar = (t / 2.0) as i64 % 4;
            let root = [220.0, 174.6, 261.6, 196.0][bar as usize];
            let chord: f64 = [1.0, 1.189, 1.498].iter().map(|m| (std::f64::consts::TAU * root * m * t).sin()).sum::<f64>() * 0.12;
            let hat_t = (t + 0.25) % 0.5;
            let hat = ((t * 91_337.0).sin() * 43_758.545).fract() * (-hat_t * 60.0).exp() * 0.3;
            (kick * 0.8 + chord + hat) as f32
        })
        .collect()
}

/// Debug aid: write one raw engine frame (before glyph conversion) as PPM.
pub fn dump(kind: Kind, t: f32, w: usize, h: usize, path: &str) -> Result<()> {
    let scenes = Scenes::new();
    let mut engine = Engine::new();
    let mut an = Analyzer::default();
    an.update(&synth(t as f64, 2048), 0.1);
    let (scene, cam) = scenes.build(kind, t, &an.bands, Orbit::default());
    let px = engine.render(&scene, &cam, w, h);
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    out.extend_from_slice(px);
    std::fs::write(path, out)?;
    Ok(())
}

pub fn run(start: Kind, mut renderer: Renderer) -> Result<()> {
    renderer.black = 0.2;
    renderer.value_density = true;
    let mut term = ratatui::init();
    let scenes = Scenes::new();
    let mut engine = Engine::new();
    let mut kind = start;
    let mut orbit = Orbit::default();
    let mut paused = false;
    let mut t = 0.0f32;
    let mut last = Instant::now();
    let mut analyzer = Analyzer::default();
    let mut cells = Vec::new();
    let (mut fps, mut frames, mut fps_at) = (0.0, 0u32, Instant::now());
    let mut render_ms = 0.0;
    let res = (|| -> Result<()> {
        loop {
            let dt = last.elapsed().as_secs_f32().min(0.1);
            last = Instant::now();
            if !paused {
                t += dt;
            }
            let (cols, rows) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize))?;
            let (w, h) = (cols, rows.saturating_sub(1).max(1));
            analyzer.update(&synth(t as f64 - 2048.0 / 48000.0, 2048), dt);
            let r0 = Instant::now();
            let (scene, cam) = scenes.build(kind, t, &analyzer.bands, orbit);
            let px = engine.render(&scene, &cam, w * CW, h * CH);
            renderer.render(&Img { w: w * CW, h: h * CH, px }, w, h, &mut cells);
            render_ms = render_ms * 0.9 + r0.elapsed().as_secs_f64() * 1000.0 * 0.1;
            frames += 1;
            if fps_at.elapsed() >= Duration::from_secs(1) {
                fps = frames as f64 / fps_at.elapsed().as_secs_f64();
                frames = 0;
                fps_at = Instant::now();
            }
            let status = format!(
                " ytui 3D · {} · {} · {fps:.0} fps ({render_ms:.1} ms/frame, {w}x{h} cells = {}x{} px) │ 1-4 scene  m mode  ←→↑↓ orbit  +- zoom  space pause  q quit ",
                kind.name(),
                renderer.mode.name(),
                w * CW,
                h * CH
            );
            term.draw(|f| {
                let buf = f.buffer_mut();
                for y in 0..h {
                    for x in 0..w {
                        let c = cells[y * w + x];
                        buf[(x as u16, y as u16)]
                            .set_char(c.ch)
                            .set_fg(Color::Rgb(c.fg[0], c.fg[1], c.fg[2]))
                            .set_bg(Color::Rgb(c.bg[0], c.bg[1], c.bg[2]));
                    }
                }
                let s: String = status.chars().take(cols).collect();
                buf.set_string(0, h as u16, format!("{s:<cols$}"), Style::default().fg(Color::White).bg(Color::Rgb(40, 0, 0)));
            })?;
            while event::poll(Duration::ZERO)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Release {
                        continue;
                    }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char(c @ '1'..='4') => kind = Kind::ALL[(c as u8 - b'1') as usize],
                        KeyCode::Char('m') => renderer.mode = renderer.mode.next(),
                        KeyCode::Char(' ') => paused = !paused,
                        KeyCode::Left => orbit.yaw -= 0.15,
                        KeyCode::Right => orbit.yaw += 0.15,
                        KeyCode::Up => orbit.pitch = (orbit.pitch + 0.08).min(1.2),
                        KeyCode::Down => orbit.pitch = (orbit.pitch - 0.08).max(-0.5),
                        KeyCode::Char('+') | KeyCode::Char('=') => orbit.zoom = (orbit.zoom * 0.9).max(0.3),
                        KeyCode::Char('-') => orbit.zoom = (orbit.zoom * 1.1).min(3.0),
                        _ => {}
                    }
                }
            }
            // Cap at ~60 fps.
            let spent = last.elapsed();
            if spent < Duration::from_millis(16) {
                std::thread::sleep(Duration::from_millis(16) - spent);
            }
        }
    })();
    ratatui::restore();
    res
}
