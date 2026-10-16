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
#[cfg(unix)]
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
    /// ffplay reading raw PCM on stdin (the Windows default).
    Ffplay,
}

impl Audio {
    pub fn auto() -> Audio {
        if cfg!(windows) {
            return if std::env::var("SSH_CONNECTION").is_ok() { Audio::None } else { Audio::Ffplay };
        }
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
    /// Play through libmpv instead of an ffmpeg pipe.
    pub mpv: bool,
    /// With mpv: render natively into this window (the overlay) instead of
    /// through the terminal.
    pub wid: Option<i64>,
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
        .args(["-i", &crate::proxy::wrap(input), "-frames:v", "1", "-vf"])
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

pub use crate::sys::die_with_parent;
use crate::sys::on_path;

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
        Audio::Ffplay if on_path("ffplay") => {
            c = Command::new("ffplay");
            c.args(["-hide_banner", "-loglevel", "quiet", "-nodisp", "-autoexit", "-fflags", "nobuffer"]);
            c.args(["-f", "s16le", "-sample_rate", "48000", "-ch_layout", "stereo", "-i", "-"]);
        }
        _ => return None,
    }
    c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    die_with_parent(&mut c);
    Some(c)
}

/// Remove audio pipes left behind by ytui processes that were killed.
pub fn clean_stale_fifos() {
    #[cfg(unix)]
    {
        let dir = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(pid) = name.strip_prefix("ytui-").and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
            if name.ends_with(".pcm") && !crate::sys::pid_alive(pid) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Where ffmpeg writes raw PCM for the audio thread: a named pipe on Unix,
/// a loopback TCP connection on Windows (which has no mkfifo).
struct PcmPipe {
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(windows)]
    listener: std::net::TcpListener,
}

impl PcmPipe {
    #[cfg(unix)]
    fn new() -> Option<PcmPipe> {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let p = dir.join(format!("ytui-{}-{}.pcm", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_file(&p);
        let ok = Command::new("mkfifo").arg("-m").arg("600").arg(&p).status().is_ok_and(|s| s.success());
        ok.then_some(PcmPipe { path: p })
    }

    #[cfg(windows)]
    fn new() -> Option<PcmPipe> {
        Some(PcmPipe { listener: std::net::TcpListener::bind("127.0.0.1:0").ok()? })
    }

    /// The output argument to give ffmpeg.
    fn target(&self) -> std::ffi::OsString {
        #[cfg(unix)]
        return self.path.clone().into_os_string();
        #[cfg(windows)]
        return format!("tcp://{}", self.listener.local_addr().unwrap()).into();
    }

    /// Wait for ffmpeg to start writing; blocks until it does or `unblock`.
    fn open(&self) -> Option<Box<dyn Read + Send>> {
        #[cfg(unix)]
        return std::fs::File::open(&self.path).ok().map(|f| Box::new(f) as _);
        #[cfg(windows)]
        return self.listener.accept().ok().map(|(c, _)| Box::new(c) as _);
    }

    /// Release an `open` that ffmpeg never satisfied, and clean up.
    fn unblock(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::OpenOptions::new().write(true).custom_flags(0o4000 /* O_NONBLOCK */).open(&self.path);
            let _ = std::fs::remove_file(&self.path);
        }
        #[cfg(windows)]
        if let Ok(a) = self.listener.local_addr() {
            let _ = std::net::TcpStream::connect(a);
        }
    }
}

struct Decoder {
    child: Child,
    sh: Arc<Shared>,
    start: f64,
    fps: f64,
    audio: Audio,
    fifo: Option<Arc<PcmPipe>>,
}

impl Decoder {
    /// Freeze playback without losing the connection: SIGSTOP stops ffmpeg
    /// decoding and writing, so nothing buffers and progress stays exact.
    /// (Gating our own reads instead lets ~1 s of frames pile up in the pipe,
    /// which then plays out on resume as a jump.)
    fn set_paused(&self, paused: bool) {
        crate::sys::suspend(&self.child, paused);
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(f) = &self.fifo {
            // Unblock the audio thread if ffmpeg never opened the pipe.
            f.unblock();
        }
    }
}

/// Volume as f32 bits, shared with the audio thread.
pub type Volume = Arc<AtomicU32>;

/// Read PCM from the fifo, keep a copy for the visualiser, scale by the
/// volume, feed the sink (if any). If the sink dies, keep draining so video
/// carries on.
fn audio_pump(fifo: Arc<PcmPipe>, sink: Option<Command>, vol: Volume, sh: Arc<Shared>) {
    std::thread::spawn(move || {
        let Some(mut f) = fifo.open() else { return };
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
        cmd.args(["-readrate", "1", "-i", &crate::proxy::wrap(url)]);
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
        match (raw_ok, PcmPipe::new()) {
            (true, Some(f)) => {
                cmd.args(["-f", "s16le", "-ac", "2", "-ar", "48000", "-y"]).arg(f.target());
                sink = player;
                fifo = Some(Arc::new(f));
            }
            _ if opts.audio == Audio::None || cfg!(windows) => {
                // (Windows ffmpeg has no audio output device of its own.)
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
    mpv: Option<crate::mpv::Engine>,
    /// mpv has loaded the current file (seeks and tracks work).
    mpv_loaded: bool,
    /// mpv has started playing it (frames are real video).
    mpv_ready: bool,
    /// A seek asked for before the file loaded.
    mpv_seek: bool,
    /// Selected caption track (index into stream.subs).
    cc: Option<usize>,
    /// mpv's track id for each caption track once added.
    cc_ids: Vec<Option<i64>>,
    /// Language to switch captions on in when the file loads.
    cc_pref: Option<String>,
    /// Captions drawn into the picture by mpv (pixel graphics) rather than
    /// as text by us.
    cc_burn: bool,
    cc_text: String,
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
            mpv: None,
            mpv_loaded: false,
            mpv_ready: false,
            mpv_seek: false,
            cc: None,
            cc_ids: Vec::new(),
            cc_pref: None,
            cc_burn: false,
            cc_text: String::new(),
        };
        p.restart();
        p
    }

    pub fn duration(&self) -> f64 {
        self.stream.duration.unwrap_or(0.0)
    }

    /// A decoder exists but has produced no frame yet.
    pub fn loading(&self) -> bool {
        (self.dec.is_some() || self.mpv.is_some()) && self.seen == 0 && !self.ended
    }

    /// Actively advancing, i.e. worth polling the terminal at full rate.
    /// False while paused even though the decoder is still alive.
    pub fn playing(&self) -> bool {
        (self.dec.is_some() || self.mpv.is_some()) && !self.paused && !self.ended
    }

    /// Decode size in pixels for the current terminal and cell size.
    fn decode_size(&self) -> (usize, usize) {
        let (w, h, _, _) = fit(self.stream.aspect, self.term.0, self.term.1);
        (((w as f32 * self.px_cell.0) as usize).max(2) & !1, ((h as f32 * self.px_cell.1) as usize).max(2) & !1)
    }

    /// (Re)load the stream into mpv at self.pos.
    fn mpv_load(&mut self) {
        if self.mpv.is_none() {
            let ao = match self.opts.audio {
                Audio::None => "null",
                Audio::Pulse => "pulse,",
                Audio::Alsa => "alsa,",
                Audio::Ffplay => "",
            };
            self.mpv = crate::mpv::Engine::new(ao, self.opts.wid);
            if self.native() {
                // mpv draws captions at full resolution in its own window.
                self.cc_burn = true;
            }
        }
        let Some(m) = &self.mpv else {
            self.note = "libmpv failed to start; using ffmpeg".into();
            self.opts.mpv = false;
            return self.restart();
        };
        self.seen = 0;
        self.exposure.reset();
        self.exp_seen = u64::MAX;
        self.ended = false;
        self.mpv_loaded = false;
        self.mpv_ready = false;
        self.mpv_seek = false;
        self.cc = None;
        self.cc_ids = vec![None; self.stream.subs.len()];
        let (pw, ph) = self.decode_size();
        if self.fw != pw || self.fh != ph {
            self.frame.clear();
        }
        (self.fw, self.fh) = (pw, ph);
        // %N% quoting: URLs may contain commas.
        let q = |v: &str| format!("%{}%{v}", v.len());
        let mut o = Vec::new();
        if !self.stream.live && self.pos > 0.25 {
            o.push(format!("start={:.3}", self.pos));
        }
        if let Some(a) = &self.stream.audio_url {
            o.push(format!("audio-files={}", q(&crate::proxy::wrap(a))));
        }
        o.push(format!("pause={}", if self.paused { "yes" } else { "no" }));
        let g = if self.muted { 0.0 } else { self.level };
        m.set_f64("volume", g as f64 * 100.0);
        if let Err(e) = m.command(&["loadfile", &crate::proxy::wrap(&self.stream.video_url), "replace", "-1", &o.join(",")]) {
            self.note = format!("mpv: {e}");
            self.failed = true;
        }
    }

    fn restart(&mut self) {
        if self.opts.mpv {
            return self.mpv_load();
        }
        self.dec = None;
        self.seen = 0;
        // A seek is a hard cut: measure the next frame afresh.
        self.exposure.reset();
        self.exp_seen = u64::MAX;
        self.ended = false;
        if self.paused {
            return;
        }
        let (pw, ph) = self.decode_size();
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
        if let Some(m) = &self.mpv {
            // mpv seeks in place and renders at any size: no reload.
            self.ended = false;
            let (pw, ph) = self.decode_size();
            if (pw, ph) != (self.fw, self.fh) {
                (self.fw, self.fh) = (pw, ph);
                self.frame.clear();
                self.seen = 0;
            }
            if self.stream.live {
                return;
            }
            if self.mpv_loaded {
                let _ = m.command(&["seek", &format!("{:.3}", self.pos), "absolute"]);
                self.exposure.reset();
                self.exp_seen = u64::MAX;
            } else {
                self.mpv_seek = true;
            }
            return;
        }
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
        if let Some(m) = &self.mpv {
            return m.set_f64("volume", g as f64 * 100.0);
        }
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
        // (mpv doesn't hand out PCM, so under mpv the visualiser idles.)
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
        if let Some(m) = &self.mpv {
            if !self.paused && self.ended {
                self.pos = 0.0;
                self.ended = false;
                if self.mpv_loaded {
                    let _ = m.command(&["seek", "0", "absolute"]);
                }
            }
            return m.set_flag("pause", self.paused);
        }
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
        if self.mpv.is_some() {
            return self.tick_mpv();
        }
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

    fn tick_mpv(&mut self) -> bool {
        let Some(m) = self.mpv.as_ref() else { return false };
        let ev = m.poll();
        let mut fresh = false;
        if ev.loaded && !self.mpv_loaded {
            self.mpv_loaded = true;
            if std::mem::take(&mut self.mpv_seek) {
                let _ = m.command(&["seek", &format!("{:.3}", self.pos), "absolute"]);
            }
            self.apply_cc_pref();
        }
        if ev.restarted {
            self.mpv_ready = true;
            if self.native() && self.seen == 0 {
                // No frames come back to us; the picture is up.
                self.seen = 1;
                fresh = true;
            }
        }
        let Some(m) = self.mpv.as_mut() else { return false };
        if self.mpv_ready {
            if let Some(t) = m.get_f64("time-pos") {
                self.pos = t;
            }
        }
        if m.frame(self.fw, self.fh, self.seen == 0 && self.mpv_ready, &mut self.frame) && self.mpv_ready {
            self.seen += 1;
            self.fps_win.1 += 1;
            fresh = true;
        }
        let el = self.fps_win.0.elapsed().as_secs_f64();
        if el >= 1.0 {
            self.fps = self.fps_win.1 as f64 / el;
            self.fps_win = (Instant::now(), 0);
        }
        if self.cc.is_some() && !self.cc_burn {
            let t = m.get_str("sub-text").unwrap_or_default();
            if t != self.cc_text {
                self.cc_text = t;
                fresh = true;
            }
        }
        // keep-open: at the end mpv holds the last frame and sets eof-reached.
        let eof = ev.eof || m.get_str("eof-reached").is_some_and(|v| v == "yes");
        if eof && !self.ended && self.mpv_loaded {
            self.ended = true;
            self.paused = true;
            fresh = true;
        }
        if let Some(e) = ev.error {
            self.note = format!("mpv: {e}");
            self.failed = true;
            self.paused = true;
            fresh = true;
        }
        fresh
    }

    /// Video is drawn by mpv in the native overlay window, not by us.
    pub fn native(&self) -> bool {
        self.mpv.is_some() && self.opts.wid.is_some()
    }

    /// Switch between the native window (Some) and terminal rendering,
    /// keeping the position and pause state.
    pub fn set_native(&mut self, wid: Option<i64>) {
        if self.opts.wid == wid || !self.opts.mpv {
            return;
        }
        let pref = self.caption_track().map(|_| self.cc.and_then(|i| self.stream.subs.get(i)).map(|t| t.lang.clone())).flatten();
        self.opts.wid = wid;
        self.mpv = None;
        self.cc_burn = false;
        if pref.is_some() {
            self.cc_pref = pref;
        }
        self.frame.clear();
        self.restart();
    }

    /// Name of the selected caption track, if captions are on.
    pub fn caption_track(&self) -> Option<&str> {
        self.cc.and_then(|i| self.stream.subs.get(i)).map(|t| t.name.as_str())
    }

    /// Caption text to draw over the video right now (empty when off, or
    /// when mpv draws them into the picture).
    pub fn caption_text(&self) -> &str {
        if self.cc.is_some() && !self.cc_burn {
            &self.cc_text
        } else {
            ""
        }
    }

    /// Have mpv draw captions into the picture (pixel graphics, where text
    /// can't sit on top of the image) or leave them to `caption_text`.
    pub fn set_caption_burn(&mut self, burn: bool) {
        let burn = burn || self.native();
        if self.cc_burn != burn {
            self.cc_burn = burn;
            if let Some(m) = &self.mpv {
                m.set_flag("sub-visibility", burn && self.cc.is_some());
            }
        }
    }

    /// Turn captions on in this language when the video loads (None: off).
    pub fn set_caption_pref(&mut self, lang: Option<String>) {
        self.cc_pref = lang;
        if self.mpv_loaded {
            self.apply_cc_pref();
        }
    }

    fn apply_cc_pref(&mut self) {
        let Some(lang) = self.cc_pref.clone() else { return };
        let subs = &self.stream.subs;
        let i = subs
            .iter()
            .position(|t| !t.auto && t.lang == lang)
            .or_else(|| subs.iter().position(|t| t.lang == lang))
            .or_else(|| subs.iter().position(|t| t.lang.split('-').next() == lang.split('-').next()));
        if let Some(i) = i {
            if let Some(e) = self.select_caption(Some(i)) {
                self.note = e;
            }
        }
    }

    /// Switch to caption track `i` (None: off). Returns an error message.
    fn select_caption(&mut self, i: Option<usize>) -> Option<String> {
        let m = self.mpv.as_ref()?;
        self.cc_text.clear();
        match i {
            None => {
                m.set("sid", "no");
                m.set_flag("sub-visibility", false);
                self.cc = None;
            }
            Some(i) => {
                let t = self.stream.subs.get(i)?;
                match self.cc_ids.get(i).copied().flatten() {
                    Some(id) => m.set("sid", &id.to_string()),
                    None => {
                        if let Err(e) = m.command(&["sub-add", &t.url, "select", &t.name, &t.lang]) {
                            return Some(format!("captions: {e}"));
                        }
                        let id = m.get_i64("sid");
                        if let Some(slot) = self.cc_ids.get_mut(i) {
                            *slot = id;
                        }
                    }
                }
                m.set_flag("sub-visibility", self.cc_burn);
                self.cc = Some(i);
            }
        }
        None
    }

    /// Next caption track (off -> each track -> off). Returns a message for
    /// the status line and the language now preferred (None: off).
    pub fn cycle_captions(&mut self) -> (String, Option<String>) {
        if self.mpv.is_none() {
            return ("captions need libmpv (see README)".into(), self.cc_pref.clone());
        }
        if self.stream.subs.is_empty() {
            return ("no captions for this video".into(), self.cc_pref.clone());
        }
        if !self.mpv_loaded {
            return ("captions: video still loading".into(), self.cc_pref.clone());
        }
        let next = match self.cc {
            None => Some(0),
            Some(i) if i + 1 < self.stream.subs.len() => Some(i + 1),
            Some(_) => None,
        };
        if let Some(e) = self.select_caption(next) {
            return (e, self.cc_pref.clone());
        }
        self.cc_pref = next.map(|i| self.stream.subs[i].lang.clone());
        let msg = match self.caption_track() {
            Some(n) => format!("captions: {n}"),
            None => "captions off".into(),
        };
        (msg, self.cc_pref.clone())
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
