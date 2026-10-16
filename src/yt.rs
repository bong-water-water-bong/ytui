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
}

pub fn ytdlp() -> String {
    if let Ok(p) = std::env::var("YTUI_YTDLP") {
        return p;
    }
    let local = std::env::var("HOME").map(|h| format!("{h}/.local/bin/yt-dlp")).unwrap_or_default();
    if Command::new("yt-dlp").arg("--version").output().is_ok() {
        "yt-dlp".into()
    } else if std::path::Path::new(&local).exists() {
        local
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
    let home = std::env::var("HOME").unwrap_or_default();
    let mut nvm: Vec<_> = std::fs::read_dir(format!("{home}/.nvm/versions/node"))
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

fn run_json(args: &[&str]) -> Result<Value> {
    static JS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let out = Command::new(ytdlp())
        .args(JS.get_or_init(js_runtime_args))
        .args(["--no-warnings", "-J"])
        .args(args)
        .output()
        .context("running yt-dlp (is it installed? set YTUI_YTDLP)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(err.trim());
        bail!("{}", line.trim());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// Resolve direct media URLs for playback, capped at `max_h` lines.
pub fn resolve(url: &str, max_h: u32) -> Result<Stream> {
    let fmt = format!(
        "bv*[height<={h}][vcodec!^=av01]+ba/bv*[height<={h}]+ba/b[height<={h}]/b",
        h = max_h
    );
    let v = run_json(&["--no-playlist", "-f", &fmt, url])?;
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
