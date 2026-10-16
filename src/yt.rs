//! yt-dlp front end: search / playlist listing and stream resolution.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Video {
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
    pub duration: Option<f64>,
    pub views: Option<u64>,
    pub live: bool,
}

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

fn run_json(args: &[&str]) -> Result<Value> {
    let out = Command::new(ytdlp())
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

fn looks_like_target(q: &str) -> bool {
    q.starts_with("http://")
        || q.starts_with("https://")
        || q.starts_with("www.")
        || q.starts_with("youtube.com")
        || q.starts_with("youtu.be")
}

fn entry(e: &Value) -> Option<Video> {
    let id = e["id"].as_str()?.to_string();
    let url = e["url"]
        .as_str()
        .or(e["webpage_url"].as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));
    Some(Video {
        title: e["title"].as_str().unwrap_or("(untitled)").to_string(),
        channel: e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("").to_string(),
        duration: e["duration"].as_f64(),
        views: e["view_count"].as_u64(),
        live: e["live_status"].as_str() == Some("is_live") || e["is_live"].as_bool() == Some(true),
        id,
        url,
    })
}

/// Search YouTube, or list a playlist / channel / single video URL.
pub fn search(q: &str, n: usize) -> Result<Vec<Video>> {
    let target = if looks_like_target(q) { q.to_string() } else { format!("ytsearch{n}:{q}") };
    let v = run_json(&["--flat-playlist", "--playlist-end", &n.max(50).to_string(), &target])?;
    let list: Vec<Video> = match v["entries"].as_array() {
        Some(es) => es.iter().filter_map(entry).filter(|v| !v.id.starts_with("UC")).collect(),
        None => entry(&v).into_iter().collect(),
    };
    if list.is_empty() {
        bail!("no results");
    }
    Ok(list)
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

pub fn fmt_views(v: u64) -> String {
    match v {
        0..=999 => format!("{v}"),
        1_000..=999_999 => format!("{:.1}K", v as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", v as f64 / 1e6),
        _ => format!("{:.1}B", v as f64 / 1e9),
    }
}
