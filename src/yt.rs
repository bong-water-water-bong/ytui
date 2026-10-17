//! yt-dlp front end: resolves direct stream URLs for playback.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Stream {
    pub title: String,
    pub video_url: String,
    pub audio_url: Option<String>,
    /// Display aspect ratio (w / h).
    pub aspect: f64,
    pub duration: Option<f64>,
    pub live: bool,
    /// Source frame rate, if known.
    pub fps: Option<f64>,
    /// Caption tracks: uploaded ones first, then auto-generated.
    pub subs: Vec<SubTrack>,
}

#[derive(Clone, Debug)]
pub struct SubTrack {
    pub lang: String,
    pub name: String,
    pub url: String,
    pub auto: bool,
}

/// Caption tracks from yt-dlp's JSON, as WebVTT. Auto-generated captions
/// come in every translation YouTube offers; keep only the original
/// ("xx-orig") and English.
fn sub_tracks(v: &Value) -> Vec<SubTrack> {
    let mut out = Vec::new();
    for (key, auto) in [("subtitles", false), ("automatic_captions", true)] {
        let Some(m) = v[key].as_object() else { continue };
        for (lang, fmts) in m {
            if lang == "live_chat" || (auto && !(lang.ends_with("-orig") || lang == "en")) {
                continue;
            }
            let Some(f) = fmts.as_array().and_then(|a| a.iter().find(|f| f["ext"] == "vtt")) else { continue };
            let Some(url) = f["url"].as_str() else { continue };
            let lang = lang.trim_end_matches("-orig").to_string();
            if auto && out.iter().any(|t: &SubTrack| t.auto && t.lang == lang) {
                continue;
            }
            let name = f["name"].as_str().unwrap_or(&lang).to_string();
            let name = if auto && !name.contains("auto") { format!("{name} (auto-generated)") } else { name };
            out.push(SubTrack { lang, name, url: url.to_string(), auto });
        }
    }
    out
}

pub fn ytdlp() -> String {
    if let Ok(p) = std::env::var("YTUI_YTDLP") {
        return p;
    }
    let local = crate::sys::home().join(".local/bin/yt-dlp");
    if Command::new("yt-dlp").arg("--version").output().is_ok() {
        "yt-dlp".into()
    } else if local.exists() {
        local.display().to_string()
    } else {
        "yt-dlp".into()
    }
}

/// yt-dlp needs a JavaScript runtime to solve YouTube's stream challenges;
/// without one it falls back to clients whose URLs throttle and expire.
/// It only looks for deno by default, so point it at node when that's what
/// is installed (including nvm installs that aren't on PATH).
fn js_runtime_args() -> Vec<String> {
    let works = |cmd: &str| Command::new(cmd).arg("--version").output().is_ok_and(|o| o.status.success());
    if works("deno") {
        return Vec::new();
    }
    if works("node") {
        return vec!["--js-runtimes".into(), "node".into()];
    }
    let mut nvm: Vec<_> = std::fs::read_dir(crate::sys::home().join(".nvm/versions/node"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("bin/node"))
        .filter(|p| p.exists())
        .collect();
    nvm.sort();
    match nvm.pop() {
        Some(p) => vec!["--js-runtimes".into(), format!("node:{}", p.display())],
        None => Vec::new(),
    }
}

/// Cookies for yt-dlp, for when YouTube asks to "confirm you're not a bot":
/// --cookies FILE, --cookies-from-browser NAME, or else cookies.txt in the
/// config directory if it exists.
static COOKIES: std::sync::RwLock<Option<Vec<String>>> = std::sync::RwLock::new(None);
/// --cookies / --cookies-from-browser given on the command line (these win
/// over the saved file and survive reloads).
static COOKIES_CLI: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();

/// Default location of an exported cookies.txt.
pub fn cookies_path() -> std::path::PathBuf {
    crate::auth::config_dir().join("cookies.txt")
}

/// Set the cookie source from the command line (call before any lookup).
pub fn set_cookies(file: Option<String>, browser: Option<String>) {
    let cli = match (file, browser) {
        (Some(f), _) => Some(vec!["--cookies".into(), f]),
        (None, Some(b)) => Some(vec!["--cookies-from-browser".into(), b]),
        (None, None) => None,
    };
    let _ = COOKIES_CLI.set(cli);
    reload_cookies();
}

/// Re-read the cookie source (after signing in or out).
pub fn reload_cookies() {
    let args = COOKIES_CLI.get().cloned().flatten().unwrap_or_else(default_cookies);
    *COOKIES.write().unwrap() = Some(args);
}

/// Whether YouTube will serve us (no bot check), using the current cookies.
pub fn check_access() -> Result<()> {
    run_json(&["--no-playlist", "-f", "b", "--flat-playlist", "https://www.youtube.com/watch?v=jNQXAC9IVRw"]).map(|_| ())
}

fn default_cookies() -> Vec<String> {
    let p = cookies_path();
    if p.is_file() {
        vec!["--cookies".into(), p.display().to_string()]
    } else {
        Vec::new()
    }
}

fn cookie_args() -> Vec<String> {
    if COOKIES.read().unwrap().is_none() {
        reload_cookies();
    }
    COOKIES.read().unwrap().clone().unwrap_or_default()
}

/// Turn yt-dlp's error output into one readable line, with advice for the
/// errors people can do something about.
fn ytdlp_error(stderr: &str) -> String {
    let line = stderr.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(stderr.trim()).trim();
    if line.contains("not a bot") || line.contains("Sign in to confirm") {
        let how = if cookie_args().is_empty() { "sign in" } else { "sign in again (your saved cookies were rejected)" };
        return format!("YouTube wants to confirm you're not a bot: {how} on the Account page (@ in the sidebar), or run ytui --signin");
    }
    if line.contains("Could not copy Chrome cookie database") || line.contains("Failed to decrypt") {
        return "can't read the browser's cookies (Chrome/Edge lock and encrypt them): export cookies.txt with a browser extension instead; see ytui --help-cookies".into();
    }
    line.to_string()
}

fn run_json(args: &[&str]) -> Result<Value> {
    static JS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let out = Command::new(ytdlp())
        .args(JS.get_or_init(js_runtime_args))
        .args(cookie_args())
        .args(["--no-warnings", "-J"])
        .args(args)
        .output()
        .context("running yt-dlp (is it installed? set YTUI_YTDLP)")?;
    if !out.status.success() {
        bail!("{}", ytdlp_error(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub const COOKIES_HELP: &str = "\
YouTube sometimes stops serving videos to a connection it thinks is a bot
(\"Sign in to confirm you're not a bot\"). It usually clears on its own after a
few hours. To keep playing, sign in:

  In ytui press @ (or click Sign in in the sidebar) and choose Sign in, or run
  ytui --signin. A browser window opens on Google's sign-in page; sign in as
  you would on any website and ytui takes it from there.

  No browser window? Use your cookie extension instead: on youtube.com click
  it and press Export / Copy while ytui is waiting (Account page, \"use my
  cookie extension\"), or paste the export into ytui with Ctrl-V.

  The sign-in is saved to  COOKIES_PATH
  Also possible:  ytui --cookies FILE   or   ytui --cookies-from-browser firefox

The cookies stay on this machine and are only passed to yt-dlp. Treat the
file like a password: it signs in to your Google account.
";

/// Resolve direct media URLs for playback: the highest resolution up to
/// `max_h` lines, then the highest frame rate (YouTube only has 60 fps at
/// 720p and up), then VP9 / H.264 (AV1 decodes slowly in software).
pub fn resolve(url: &str, max_h: u32) -> Result<Stream> {
    if std::path::Path::new(url).is_file() {
        return local(url);
    }
    let sort = format!("res:{max_h},fps,vcodec:vp9");
    let v = run_json(&["--no-playlist", "-f", "bv*[vcodec!^=av01]+ba/bv*+ba/b", "-S", &sort, url])?;
    let (mut video, mut audio) = (None, None);
    if let Some(fs) = v["requested_formats"].as_array() {
        for f in fs {
            let u = f["url"].as_str().map(String::from);
            if f["vcodec"].as_str().is_some_and(|c| c != "none") {
                video = u;
            } else if f["acodec"].as_str().is_some_and(|c| c != "none") {
                audio = u;
            }
        }
    } else {
        video = v["url"].as_str().map(String::from);
    }
    let video_url = video.ok_or_else(|| anyhow!("no playable video stream"))?;
    let (w, h) = (v["width"].as_f64().unwrap_or(16.0), v["height"].as_f64().unwrap_or(9.0));
    Ok(Stream {
        title: v["title"].as_str().unwrap_or("").to_string(),
        video_url,
        audio_url: audio,
        aspect: if h > 0.0 { w / h } else { 16.0 / 9.0 },
        duration: v["duration"].as_f64(),
        live: v["is_live"].as_bool() == Some(true),
        fps: v["fps"].as_f64().filter(|f| *f > 0.0),
        subs: sub_tracks(&v),
    })
}

pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A local media file (no yt-dlp): size, length and frame rate from ffprobe.
fn local(path: &str) -> Result<Stream> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate:format=duration", "-of", "json", path])
        .output()
        .context("running ffprobe")?;
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let s = &v["streams"][0];
    let (w, h) = (s["width"].as_f64().unwrap_or(16.0), s["height"].as_f64().unwrap_or(9.0));
    let fps = s["avg_frame_rate"].as_str().and_then(|r| {
        let (a, b) = r.split_once('/')?;
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (b > 0.0).then(|| a / b)
    });
    Ok(Stream {
        title: std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        video_url: path.to_string(),
        audio_url: None,
        aspect: if h > 0.0 { w / h } else { 16.0 / 9.0 },
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        live: false,
        fps: fps.filter(|f| *f > 0.0),
        subs: Vec::new(),
    })
}
