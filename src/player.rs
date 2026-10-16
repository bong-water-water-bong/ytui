//! Playback: one ffmpeg process per play segment. ffmpeg reads the stream at
//! real-time rate (-readrate 1), sends audio straight to the sound server and
//! raw RGB frames down a pipe. A reader thread keeps only the newest frame, so
//! a slow terminal drops frames instead of drifting out of sync with audio.
//! Pause / seek / resize simply restart ffmpeg at the current position.

use crate::glyphs::{CH, CW};
use crate::render::{Cell, Img, Renderer};
use crate::term::Screen;
use crate::yt::{fmt_time, Stream};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Audio {
    None,
    Pulse,
    Alsa,
}

impl Audio {
    pub fn auto() -> Audio {
        let pulse_sock = std::env::var("XDG_RUNTIME_DIR")
            .map(|d| std::path::Path::new(&d).join("pulse/native").exists())
            .unwrap_or(false);
        if std::env::var("PULSE_SERVER").is_ok() || pulse_sock {
            Audio::Pulse
        } else if std::env::var("SSH_CONNECTION").is_ok() {
            // Don't blast the remote machine's speakers by surprise.
            Audio::None
        } else {
            Audio::Alsa
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub audio: Audio,
    pub fps: u32,
}

pub enum Exit {
    Back,
    Ended,
    Next,
    Prev,
    Quit,
}

/// Fit an aspect ratio into a cols x rows cell box; returns (w, h, x0, y0).
pub fn fit(aspect: f64, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let (pw, ph) = ((cols * CW) as f64, (rows * CH) as f64);
    let (w, h) = if pw / ph > aspect {
        (((ph * aspect) / CW as f64).round() as usize, rows)
    } else {
        (cols, ((pw / aspect) / CH as f64).round() as usize)
    };
    let (w, h) = (w.clamp(1, cols.max(1)), h.clamp(1, rows.max(1)));
    (w, h, (cols - w) / 2, (rows - h) / 2)
}

/// Decode a single frame (image URL, file, or stream) at exactly w x h px.
pub fn grab_frame(input: &str, at: f64, w: usize, h: usize) -> Option<Vec<u8>> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    if at > 0.0 {
        cmd.args(["-ss", &format!("{at:.3}")]);
    }
    let out = cmd
        .args(["-i", input, "-frames:v", "1", "-vf"])
        .arg(format!("scale={w}:{h}:flags=area,format=rgb24"))
        .args(["-f", "rawvideo", "pipe:1"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.stdout.len() == w * h * 3).then_some(out.stdout)
}

struct Shared {
    frame: Mutex<Vec<u8>>,
    count: AtomicU64,
    done: AtomicBool,
    err: Mutex<String>,
}

struct Decoder {
    child: Child,
    sh: Arc<Shared>,
    start: f64,
    fps: f64,
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    audio: Audio,
}

impl Decoder {
    fn pos(&self) -> f64 {
        self.start + self.sh.count.load(Ordering::Relaxed) as f64 / self.fps
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(s: &Stream, pos: f64, cols: usize, rows: usize, opts: Opts) -> Result<Decoder> {
    let (w, h, x0, y0) = fit(s.aspect, cols, rows);
    // Never upsample past the source frame rate.
    let fps = s.fps.map_or(opts.fps as f64, |f| f.min(opts.fps as f64));
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    let seek = if !s.live && pos > 0.25 { Some(format!("{pos:.3}")) } else { None };
    let input = |cmd: &mut Command, url: &str| {
        if let Some(ss) = &seek {
            cmd.args(["-ss", ss]);
        }
        cmd.args(["-readrate", "1", "-i", url]);
    };
    input(&mut cmd, &s.video_url);
    let audio_map = match (opts.audio, &s.audio_url) {
        (Audio::None, _) => None,
        (_, Some(a)) => {
            input(&mut cmd, a);
            Some("1:a:0")
        }
        (_, None) => Some("0:a:0?"),
    };
    cmd.args(["-map", "0:v:0", "-vf"])
        .arg(format!("fps={fps:.3},scale={}:{}:flags=area,format=rgb24", w * CW, h * CH))
        .args(["-f", "rawvideo", "pipe:1"]);
    if let Some(m) = audio_map {
        cmd.args(["-map", m]);
        match opts.audio {
            Audio::Pulse => cmd.args(["-f", "pulse", "ytui"]),
            _ => cmd.args(["-f", "alsa", "default"]),
        };
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let sh = Arc::new(Shared {
        frame: Mutex::new(Vec::new()),
        count: AtomicU64::new(0),
        done: AtomicBool::new(false),
        err: Mutex::new(String::new()),
    });
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let size = w * CW * h * CH * 3;
    let shr = sh.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; size];
        while stdout.read_exact(&mut buf).is_ok() {
            let mut f = shr.frame.lock().unwrap();
            f.resize(size, 0);
            std::mem::swap(&mut *f, &mut buf);
            drop(f);
            shr.count.fetch_add(1, Ordering::Release);
        }
        shr.done.store(true, Ordering::Release);
    });
    let she = sh.clone();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        *she.err.lock().unwrap() = s;
    });
    Ok(Decoder { child, sh, start: if seek.is_some() { pos } else { 0.0 }, fps, w, h, x0, y0, audio: opts.audio })
}

fn term_size() -> (usize, usize) {
    let (c, r) = crossterm::terminal::size().unwrap_or((80, 24));
    (c.max(10) as usize, r.max(4) as usize)
}

pub fn play(s: &Stream, r: &mut Renderer, mut opts: Opts) -> Result<Exit> {
    let mut screen = Screen::new();
    let mut cells: Vec<Cell> = Vec::new();
    let mut frame: Vec<u8> = Vec::new();
    let mut note = String::new();
    let (mut cols, mut rows) = term_size();
    let mut pos = 0.0f64;
    let mut paused = false;
    let mut dec: Option<Decoder>;
    let mut seen: u64;
    let mut redraw = false;
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut fps_win = (Instant::now(), 0u32, 0.0f64);
    let clear = || {
        let mut o = std::io::stdout();
        let _ = write!(o, "\x1b[0m\x1b[2J");
        let _ = o.flush();
    };
    let dur = s.duration.unwrap_or(0.0);

    macro_rules! restart {
        () => {{
            dec = None;
            seen = 0;
            clear();
            screen.invalidate();
            frame.clear();
            if !paused {
                dec = Some(spawn(s, pos, cols, rows - 1, opts)?);
            }
        }};
    }
    restart!();

    loop {
        if let Some(d) = &dec {
            pos = d.pos();
        }
        if event::poll(Duration::from_millis(3))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let seek = |p: f64, by: f64| if dur > 0.0 { (p + by).clamp(0.0, (dur - 1.0).max(0.0)) } else { (p + by).max(0.0) };
                    match k.code {
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Backspace => return Ok(Exit::Back),
                        KeyCode::Char('Q') => return Ok(Exit::Quit),
                        KeyCode::Char('n') => return Ok(Exit::Next),
                        KeyCode::Char('p') => return Ok(Exit::Prev),
                        KeyCode::Char(' ') | KeyCode::Char('k') => {
                            paused = !paused;
                            if paused {
                                dec = None;
                            } else {
                                restart!();
                            }
                        }
                        KeyCode::Left | KeyCode::Char('h') if !s.live => { pos = seek(pos, -10.0); restart!(); }
                        KeyCode::Right | KeyCode::Char('l') if !s.live => { pos = seek(pos, 10.0); restart!(); }
                        KeyCode::Down | KeyCode::Char('j') if !s.live => { pos = seek(pos, -60.0); restart!(); }
                        KeyCode::Up if !s.live => { pos = seek(pos, 60.0); restart!(); }
                        KeyCode::Char(c @ '0'..='9') if dur > 0.0 => {
                            pos = dur * (c as u8 - b'0') as f64 / 10.0;
                            restart!();
                        }
                        KeyCode::Char('m') => { r.mode = r.mode.next(); redraw = true; }
                        KeyCode::Char('b') => { r.ascii_bg = !r.ascii_bg; redraw = true; }
                        KeyCode::Char('[') => { r.glob_exp = (r.glob_exp - 0.2).max(1.0); redraw = true; }
                        KeyCode::Char(']') => { r.glob_exp += 0.2; redraw = true; }
                        KeyCode::Char('{') => { r.dir_exp = (r.dir_exp - 0.25).max(1.0); redraw = true; }
                        KeyCode::Char('}') => { r.dir_exp += 0.25; redraw = true; }
                        _ => {}
                    }
                    last_status = Instant::now() - Duration::from_secs(1);
                }
                Event::Resize(..) => {
                    (cols, rows) = term_size();
                    restart!();
                    if paused {
                        redraw = true;
                    }
                }
                _ => {}
            }
        }

        let (w, h, x0, y0) = match &dec {
            Some(d) => (d.w, d.h, d.x0, d.y0),
            None => {
                let f = fit(s.aspect, cols, rows - 1);
                (f.0, f.1, f.2, f.3)
            }
        };
        let mut new_frame = false;
        if let Some(d) = &dec {
            let n = d.sh.count.load(Ordering::Acquire);
            if n != seen {
                seen = n;
                std::mem::swap(&mut frame, &mut *d.sh.frame.lock().unwrap());
                new_frame = true;
                fps_win.1 += 1;
            }
        }
        if (new_frame || redraw) && frame.len() == w * CW * h * CH * 3 {
            r.render(&Img { w: w * CW, h: h * CH, px: &frame }, w, h, &mut cells);
            screen.draw(&cells, w, h, x0, y0)?;
        }
        redraw = false;

        if let Some(d) = &dec {
            if d.sh.done.load(Ordering::Acquire) && d.sh.count.load(Ordering::Acquire) == seen {
                let err = d.sh.err.lock().unwrap().clone();
                let audio_err = err.contains("pulse") || err.contains("alsa") || err.contains("ALSA") || err.contains("audio");
                if seen == 0 && d.audio != Audio::None && audio_err {
                    note = "audio output failed; playing muted".into();
                    opts.audio = Audio::None;
                    restart!();
                    continue;
                }
                if seen == 0 && !err.trim().is_empty() {
                    note = err.lines().last().unwrap_or("").trim().to_string();
                    dec = None;
                    paused = true;
                } else if !s.live && (dur == 0.0 || pos >= dur - 2.0) {
                    return Ok(Exit::Ended);
                } else if !err.trim().is_empty() {
                    note = err.lines().last().unwrap_or("").trim().to_string();
                    dec = None;
                    paused = true;
                } else {
                    return Ok(Exit::Ended);
                }
            }
        }

        if last_status.elapsed() >= Duration::from_millis(250) {
            let el = fps_win.0.elapsed().as_secs_f64();
            if el >= 1.0 {
                fps_win = (Instant::now(), 0, fps_win.1 as f64 / el);
            }
            let loading = dec.as_ref().is_some_and(|_| seen == 0);
            status_line(s, r, pos, dur, paused, loading, fps_win.2, opts, &note, cols, rows)?;
            last_status = Instant::now();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn status_line(
    s: &Stream,
    r: &Renderer,
    pos: f64,
    dur: f64,
    paused: bool,
    loading: bool,
    fps: f64,
    opts: Opts,
    note: &str,
    cols: usize,
    rows: usize,
) -> std::io::Result<()> {
    let icon = if loading { "…" } else if paused { "⏸" } else { "▶" };
    let time = if s.live {
        format!("LIVE {}", fmt_time(pos))
    } else {
        format!("{} / {}", fmt_time(pos), fmt_time(dur))
    };
    let mut right = format!(
        " {} {:.0}fps{} │ m mode  ␣ pause  ←→ seek  [] contrast  n/p next/prev  q back ",
        r.mode.name(),
        fps,
        if opts.audio == Audio::None { " muted" } else { "" }
    );
    if !note.is_empty() {
        right = format!(" {note} │{right}");
    }
    let left = format!(" {icon} {time} ");
    let bar_w = 24usize;
    let filled = if dur > 0.0 { ((pos / dur).clamp(0.0, 1.0) * bar_w as f64) as usize } else { 0 };
    let bar: String = "━".repeat(filled) + &"─".repeat(bar_w - filled);
    let title_room = cols.saturating_sub(left.chars().count() + bar_w + right.chars().count() + 2);
    let title: String = s.title.chars().take(title_room).collect();
    let mut line = format!("{left}{bar} {title}");
    let pad = cols.saturating_sub(line.chars().count() + right.chars().count());
    line.push_str(&" ".repeat(pad));
    line.push_str(&right);
    let line: String = line.chars().take(cols).collect();
    let mut o = std::io::stdout().lock();
    write!(o, "\x1b[{};1H\x1b[0;38;2;230;230;230;48;2;40;0;0m{}\x1b[0m", rows, line)?;
    o.flush()
}
