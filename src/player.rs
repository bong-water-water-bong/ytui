//! Video player component. One ffmpeg process per play segment reads the
//! stream at real-time rate (-readrate 1), sends audio to the sound server
//! and raw RGB frames down a pipe. A reader thread keeps only the newest
//! frame, so a slow terminal drops frames instead of drifting from audio.
//!
//! Frames are decoded once at full-terminal resolution; smaller views
//! (inline on the watch page, miniplayer) are downsampled here, so switching
//! views is instant. Pause / seek / terminal resize restart ffmpeg.

use crate::api::VideoItem;
use crate::glyphs::{CH, CW};
use crate::render::{Cell, Exposure, Img, Renderer};
use crate::yt::Stream;
use anyhow::Result;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

/// Fit an aspect ratio into a cols x rows cell box; returns (w, h, x0, y0).
pub fn fit(aspect: f64, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let (pw, ph) = ((cols * CW) as f64, (rows * CH) as f64);
    let (w, h) = if pw / ph > aspect {
        (((ph * aspect) / CW as f64).round() as usize, rows)
    } else {
        (cols, ((pw / aspect) / CH as f64).round() as usize)
    };
    let (w, h) = (w.clamp(1, cols), h.clamp(1, rows));
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
    audio_failed: AtomicBool,
    /// Newest mono samples (48 kHz) for the visualiser.
    pcm: Mutex<std::collections::VecDeque<f32>>,
}

const PCM_KEEP: usize = 4096;

/// Have the kernel kill the child if ytui dies (terminal closed, SIGKILL),
/// so ffmpeg / audio players never outlive us.
pub fn die_with_parent(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
}

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// Player for raw s16le 48 kHz stereo on stdin, so we can apply volume
/// ourselves. None means fall back to ffmpeg's own audio output.
fn sink_cmd(audio: Audio) -> Option<Command> {
    let mut c;
    match audio {
        Audio::Pulse if on_path("pw-cat") => {
            c = Command::new("pw-cat");
            c.args(["--playback", "--format", "s16", "--rate", "48000", "--channels", "2", "--latency", "80ms", "-"]);
        }
        Audio::Pulse if on_path("pacat") => {
            c = Command::new("pacat");
            c.args(["--raw", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=80"]);
        }
        Audio::Alsa | Audio::Pulse if on_path("aplay") => {
            c = Command::new("aplay");
            c.args(["-q", "-t", "raw", "-f", "S16_LE", "-c", "2", "-r", "48000", "--buffer-time=100000"]);
        }
        _ => return None,
    }
    c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    die_with_parent(&mut c);
    Some(c)
}

/// Remove audio pipes left behind by ytui processes that were killed.
pub fn clean_stale_fifos() {
    let dir = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pid) = name.strip_prefix("ytui-").and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if name.ends_with(".pcm") && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn make_fifo() -> Option<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let p = dir.join(format!("ytui-{}-{}.pcm", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&p);
    let ok = Command::new("mkfifo").arg("-m").arg("600").arg(&p).status().is_ok_and(|s| s.success());
    ok.then_some(p)
}

struct Decoder {
    child: Child,
    sh: Arc<Shared>,
    start: f64,
    fps: f64,
    audio: Audio,
    fifo: Option<PathBuf>,
}

impl Decoder {
    /// Freeze playback without losing the connection: SIGSTOP stops ffmpeg
    /// decoding and writing, so nothing buffers and progress stays exact.
    /// (Gating our own reads instead lets ~1 s of frames pile up in the pipe,
    /// which then plays out on resume as a jump.)
    fn set_paused(&self, paused: bool) {
        let sig = if paused { libc::SIGSTOP } else { libc::SIGCONT };
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, sig);
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(f) = &self.fifo {
            // Unblock the audio thread if ffmpeg never opened the pipe.
            let _ = std::fs::OpenOptions::new().write(true).custom_flags_nonblock().open(f);
            let _ = std::fs::remove_file(f);
        }
    }
}

trait NonBlock {
    fn custom_flags_nonblock(&mut self) -> &mut Self;
}

impl NonBlock for std::fs::OpenOptions {
    fn custom_flags_nonblock(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(0o4000) // O_NONBLOCK
    }
}

/// Volume as f32 bits, shared with the audio thread.
pub type Volume = Arc<AtomicU32>;

/// Read PCM from the fifo, keep a copy for the visualiser, scale by the
/// volume, feed the sink (if any). If the sink dies, keep draining so video
/// carries on.
fn audio_pump(fifo: PathBuf, sink: Option<Command>, vol: Volume, sh: Arc<Shared>) {
    std::thread::spawn(move || {
        let Ok(mut f) = std::fs::File::open(&fifo) else { return };
        let had_sink = sink.is_some();
        let mut child = sink.and_then(|mut s| s.spawn().ok());
        if had_sink && child.is_none() {
            sh.audio_failed.store(true, Ordering::Relaxed);
        }
        let mut buf = vec![0u8; 4096];
        while f.read_exact(&mut buf).is_ok() {
            {
                let mut pcm = sh.pcm.lock().unwrap();
                for fr in buf.chunks_exact(4) {
                    let l = i16::from_le_bytes([fr[0], fr[1]]) as f32;
                    let r = i16::from_le_bytes([fr[2], fr[3]]) as f32;
                    pcm.push_back((l + r) / 65536.0);
                }
                while pcm.len() > PCM_KEEP {
                    pcm.pop_front();
                }
            }
            let g = f32::from_bits(vol.load(Ordering::Relaxed));
            if (g - 1.0).abs() > 1e-3 {
                for s in buf.chunks_exact_mut(2) {
                    let v = (i16::from_le_bytes([s[0], s[1]]) as f32 * g).clamp(-32768.0, 32767.0) as i16;
                    s.copy_from_slice(&v.to_le_bytes());
                }
            }
            if let Some(c) = &mut child {
                if c.stdin.as_mut().is_none_or(|i| i.write_all(&buf).is_err()) {
                    sh.audio_failed.store(true, Ordering::Relaxed);
                    let _ = c.kill();
                    let _ = c.wait();
                    child = None;
                }
            }
        }
        if let Some(mut c) = child {
            drop(c.stdin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    });
}

fn spawn(s: &Stream, pos: f64, pw: usize, ph: usize, opts: Opts, vol: &Volume) -> Result<Decoder> {
    let fps = s.fps.map_or(opts.fps as f64, |f| f.min(opts.fps as f64));
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    let seek = if !s.live && pos > 0.25 { Some(format!("{pos:.3}")) } else { None };
    let input = |cmd: &mut Command, url: &str| {
        if let Some(ss) = &seek {
            cmd.args(["-ss", ss]);
        }
        if url.starts_with("http") {
            cmd.args(["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "4"]);
        }
        cmd.args(["-readrate", "1", "-i", url]);
    };
    input(&mut cmd, &s.video_url);
    // Audio is always decoded when the stream has it: played through the
    // sink, or (sound off) only analysed for the visualiser.
    let audio_map = match &s.audio_url {
        Some(a) => {
            input(&mut cmd, a);
            Some("1:a:0")
        }
        None => Some("0:a:0?"),
    };
    cmd.args(["-map", "0:v:0", "-vf"])
        .arg(format!("fps={fps:.3},scale={pw}:{ph}:flags=area,format=rgb24"))
        .args(["-f", "rawvideo", "pipe:1"]);
    let mut fifo = None;
    let mut sink = None;
    if let Some(m) = audio_map {
        cmd.args(["-map", m]);
        let player = sink_cmd(opts.audio);
        let raw_ok = player.is_some() || opts.audio == Audio::None;
        match (raw_ok, make_fifo()) {
            (true, Some(f)) => {
                cmd.args(["-f", "s16le", "-ac", "2", "-ar", "48000", "-y"]).arg(&f);
                sink = player;
                fifo = Some(f);
            }
            _ if opts.audio == Audio::None => {
                cmd.args(["-f", "null", "-"]);
            }
            _ => {
                // No raw player available: ffmpeg plays it, volume fixed per segment.
                let g = f32::from_bits(vol.load(Ordering::Relaxed));
                cmd.args(["-af", &format!("volume={g:.2}")]);
                match opts.audio {
                    Audio::Pulse => cmd.args(["-f", "pulse", "ytui"]),
                    _ => cmd.args(["-f", "alsa", "default"]),
                };
            }
        }
    }
    die_with_parent(&mut cmd);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let sh = Arc::new(Shared {
        frame: Mutex::new(Vec::new()),
        count: AtomicU64::new(0),
        done: AtomicBool::new(false),
        err: Mutex::new(String::new()),
        audio_failed: AtomicBool::new(false),
        pcm: Mutex::new(std::collections::VecDeque::with_capacity(PCM_KEEP)),
    });
    if let Some(f) = &fifo {
        audio_pump(f.clone(), sink, vol.clone(), sh.clone());
    }
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let size = pw * ph * 3;
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
    Ok(Decoder { child, sh, start: if seek.is_some() { pos } else { 0.0 }, fps, audio: opts.audio, fifo })
}

pub struct Player {
    pub video: VideoItem,
    pub stream: Stream,
    pub opts: Opts,
    pub pos: f64,
    pub paused: bool,
    pub ended: bool,
    pub note: String,
    /// The stream failed part-way (e.g. the URL expired); worth re-resolving.
    pub failed: bool,
    vol: Volume,
    level: f32,
    muted: bool,
    dec: Option<Decoder>,
    /// Decoded frame and its size in pixels.
    frame: Vec<u8>,
    fw: usize,
    fh: usize,
    seen: u64,
    /// Cell box the decode resolution was chosen for (the full terminal).
    term: (usize, usize),
    scratch: Vec<u8>,
    cells: Vec<Cell>,
    pub exposure: Exposure,
    /// Frame number the exposure was last measured on.
    exp_seen: u64,
    /// Decoded pixels per terminal cell (6x12 for glyphs; more for pixel
    /// graphics backends).
    px_cell: (f32, f32),
    pix: Vec<u8>,
    fps_win: (Instant, u32),
    pub fps: f64,
}

impl Player {
    pub fn new(video: VideoItem, stream: Stream, start: f64, opts: Opts, term: (usize, usize), volume: f32, exposure: bool) -> Player {
        let mut p = Player {
            video,
            stream,
            opts,
            pos: start,
            paused: false,
            ended: false,
            note: String::new(),
            failed: false,
            vol: Arc::new(AtomicU32::new(volume.to_bits())),
            level: volume,
            muted: false,
            dec: None,
            frame: Vec::new(),
            fw: 0,
            fh: 0,
            seen: 0,
            term,
            scratch: Vec::new(),
            cells: Vec::new(),
            exposure: Exposure::new(exposure),
            exp_seen: u64::MAX,
            px_cell: (CW as f32, CH as f32),
            pix: Vec::new(),
            fps_win: (Instant::now(), 0),
            fps: 0.0,
        };
        p.restart();
        p
    }

    pub fn duration(&self) -> f64 {
        self.stream.duration.unwrap_or(0.0)
    }

    /// A decoder exists but has produced no frame yet.
    pub fn loading(&self) -> bool {
        self.dec.is_some() && self.seen == 0
    }

    /// Actively advancing, i.e. worth polling the terminal at full rate.
    /// False while paused even though the decoder is still alive.
    pub fn playing(&self) -> bool {
        self.dec.is_some() && !self.paused && !self.ended
    }

    fn restart(&mut self) {
        self.dec = None;
        self.seen = 0;
        // A seek is a hard cut: measure the next frame afresh.
        self.exposure.reset();
        self.exp_seen = u64::MAX;
        self.ended = false;
        if self.paused {
            return;
        }
        let (w, h, _, _) = fit(self.stream.aspect, self.term.0, self.term.1);
        let (pw, ph) = (((w as f32 * self.px_cell.0) as usize).max(2) & !1, ((h as f32 * self.px_cell.1) as usize).max(2) & !1);
        match spawn(&self.stream, self.pos, pw, ph, self.opts, &self.vol) {
            Ok(d) => {
                self.dec = Some(d);
                if self.fw != pw || self.fh != ph {
                    self.frame.clear();
                }
                self.fw = pw;
                self.fh = ph;
            }
            Err(e) => {
                self.note = format!("ffmpeg: {e}");
                self.paused = true;
            }
        }
    }

    /// Replace the decoder with one starting at `pos`, keeping pause state.
    /// A seek always needs a fresh ffmpeg (an HTTP stream can't be scrubbed),
    /// so the new one comes up already frozen when the old one was paused.
    fn respawn(&mut self) {
        let was_paused = self.paused;
        self.paused = false;
        self.restart();
        if was_paused {
            self.paused = true;
            if let Some(d) = self.dec.as_ref() {
                d.set_paused(true);
            }
        }
    }

    /// The user's volume level (kept while muted).
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Push the effective gain to the audio thread.
    fn apply_volume(&mut self) {
        let g = if self.muted { 0.0 } else { self.level };
        self.vol.store(g.to_bits(), Ordering::Relaxed);
        // Without our own audio pump the gain is baked into ffmpeg's filter.
        if self.dec.as_ref().is_some_and(|d| d.fifo.is_none() && d.audio != Audio::None) {
            self.restart();
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.level = v.clamp(0.0, 1.5);
        self.muted = false;
        self.apply_volume();
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.apply_volume();
    }

    /// Decode at this many pixels per terminal cell (restarts if changed).
    pub fn set_px_cell(&mut self, px: (f32, f32)) {
        if (self.px_cell.0 - px.0).abs() > 0.01 || (self.px_cell.1 - px.1).abs() > 0.01 {
            self.px_cell = px;
            self.respawn();
        }
    }

    /// The current frame at decoded resolution with auto-exposure applied,
    /// plus its frame number: (rgb, w, h, seq).
    pub fn pixels(&mut self) -> Option<(&[u8], usize, usize, u64)> {
        if self.fw == 0 || self.frame.len() != self.fw * self.fh * 3 {
            return None;
        }
        if self.exposure.on && self.seen != self.exp_seen {
            self.exposure.measure(&self.frame, 0.15);
            self.exp_seen = self.seen;
        }
        self.pix.clear();
        self.pix.extend_from_slice(&self.frame);
        self.exposure.apply(&mut self.pix);
        Some((&self.pix, self.fw, self.fh, self.seen))
    }

    /// Newest mono samples for the visualiser (empty when there's no audio).
    pub fn samples(&self) -> Vec<f32> {
        self.dec.as_ref().map(|d| d.sh.pcm.lock().unwrap().iter().copied().collect()).unwrap_or_default()
    }

    pub fn set_term(&mut self, cols: usize, rows: usize) {
        if self.term != (cols, rows) {
            self.term = (cols, rows);
            self.respawn();
        }
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        match self.dec.as_ref() {
            // Freeze in place; resuming is instant and the position is exact.
            Some(d) => d.set_paused(self.paused),
            // Paused with no decoder (opened paused, or it exited) -> start one.
            None if !self.paused => {
                if self.ended {
                    self.pos = 0.0;
                }
                self.restart();
            }
            None => {}
        }
    }

    pub fn seek_to(&mut self, t: f64) {
        if self.stream.live {
            return;
        }
        let d = self.duration();
        self.pos = if d > 0.0 { t.clamp(0.0, (d - 1.0).max(0.0)) } else { t.max(0.0) };
        self.respawn();
    }

    pub fn seek_by(&mut self, by: f64) {
        self.seek_to(self.pos + by);
    }

    /// Advance; returns true when something visible changed.
    pub fn tick(&mut self) -> bool {
        let Some(d) = &self.dec else { return false };
        let n = d.sh.count.load(Ordering::Acquire);
        self.pos = d.start + n as f64 / d.fps;
        let mut fresh = false;
        if n != self.seen {
            self.seen = n;
            std::mem::swap(&mut self.frame, &mut *d.sh.frame.lock().unwrap());
            fresh = true;
            self.fps_win.1 += 1;
        }
        let el = self.fps_win.0.elapsed().as_secs_f64();
        if el >= 1.0 {
            self.fps = self.fps_win.1 as f64 / el;
            self.fps_win = (Instant::now(), 0);
        }
        if d.sh.audio_failed.swap(false, Ordering::Relaxed) {
            self.note = "audio output failed; playing muted".into();
            fresh = true;
        }
        if d.sh.done.load(Ordering::Acquire) && d.sh.count.load(Ordering::Acquire) == self.seen {
            let err = d.sh.err.lock().unwrap().trim().to_string();
            let audio_err = ["pulse", "alsa", "ALSA", "audio"].iter().any(|k| err.contains(k));
            if self.seen == 0 && d.audio != Audio::None && audio_err {
                self.note = "audio output failed; playing muted".into();
                self.opts.audio = Audio::None;
                self.restart();
                return true;
            }
            let near_end = !self.stream.live && (self.duration() == 0.0 || self.pos >= self.duration() - 2.0);
            if !err.is_empty() && !near_end {
                self.note = err.lines().last().unwrap_or("").to_string();
                self.failed = true;
            }
            self.dec = None;
            self.paused = true;
            self.ended = near_end || err.is_empty();
            return true;
        }
        fresh
    }

    /// Render the current frame fitted into a w x h cell box.
    /// Returns (cells, fitted w, fitted h, x offset, y offset).
    pub fn cells(&mut self, r: &mut Renderer, w: usize, h: usize) -> Option<(&[Cell], usize, usize, usize, usize)> {
        if self.fw == 0 || self.frame.len() != self.fw * self.fh * 3 {
            return None;
        }
        let (cw, ch, x0, y0) = fit(self.fw as f64 / self.fh as f64, w, h);
        let (tw, th) = (cw * CW, ch * CH);
        if self.exposure.on && self.seen != self.exp_seen {
            // Once per decoded frame; ~0.15 per frame eases in over ~0.2 s.
            self.exposure.measure(&self.frame, 0.15);
            self.exp_seen = self.seen;
        }
        if (tw, th) == (self.fw, self.fh) {
            self.scratch.clear();
            self.scratch.extend_from_slice(&self.frame);
        } else {
            downsample(&self.frame, self.fw, self.fh, tw, th, &mut self.scratch);
        }
        self.exposure.apply(&mut self.scratch);
        // Video: keep art-mode characters steady between frames.
        r.temporal = true;
        r.render(&Img { w: tw, h: th, px: &self.scratch }, cw, ch, &mut self.cells);
        r.temporal = false;
        Some((&self.cells, cw, ch, x0, y0))
    }
}

/// Area-average resample (nearest source pixel when upscaling).
pub fn downsample(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(dw * dh * 3, 0);
    let xs: Vec<(usize, usize)> = (0..dw)
        .map(|x| {
            let a = (x * sw / dw).min(sw - 1);
            (a, ((x + 1) * sw / dw).clamp(a + 1, sw))
        })
        .collect();
    for y in 0..dh {
        let y0 = (y * sh / dh).min(sh - 1);
        let y1 = ((y + 1) * sh / dh).clamp(y0 + 1, sh);
        for (x, &(x0, x1)) in xs.iter().enumerate() {
            let mut s = [0u32; 3];
            for yy in y0..y1 {
                let row = &src[(yy * sw + x0) * 3..(yy * sw + x1) * 3];
                for p in row.chunks_exact(3) {
                    s[0] += p[0] as u32;
                    s[1] += p[1] as u32;
                    s[2] += p[2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = &mut out[(y * dw + x) * 3..(y * dw + x) * 3 + 3];
            o[0] = (s[0] / n) as u8;
            o[1] = (s[1] / n) as u8;
            o[2] = (s[2] / n) as u8;
        }
    }
}
