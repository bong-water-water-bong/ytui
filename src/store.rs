//! Local state: subscriptions, watch history (with resume positions) and
//! watch later, kept in ~/.local/share/ytui/state.json.

use crate::api::{now_secs, VideoItem};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
    /// YouTube subscription id when the subscription exists on your account.
    #[serde(default)]
    pub sub_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistEntry {
    pub video: VideoItem,
    pub pos: f64,
    pub dur: f64,
    pub at: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub subs: Vec<ChannelRef>,
    #[serde(default)]
    pub history: Vec<HistEntry>,
    #[serde(default)]
    pub later: Vec<VideoItem>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default = "yes")]
    pub autoplay: bool,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default = "yes")]
    pub show_chat: bool,
    /// Auto-exposure for video.
    #[serde(default = "yes")]
    pub exposure: bool,
    /// Look panel: brightness, saturation, background (how much picture
    /// shows behind the glyphs) and contrast (the shape exponent).
    #[serde(default = "look_bright")]
    pub look_bright: f32,
    #[serde(default = "look_sat")]
    pub look_sat: f32,
    #[serde(default = "look_bg")]
    pub look_bg: f32,
    #[serde(default = "look_contrast")]
    pub look_contrast: f32,
    /// Caption language to switch on for every video (None: captions off).
    #[serde(default)]
    pub captions: Option<String>,
    #[serde(skip)]
    dirty: bool,
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}

// The look defaults are the react-video-ascii demo's.
fn look_bright() -> f32 {
    1.25
}

fn look_sat() -> f32 {
    1.25
}

fn look_bg() -> f32 {
    0.30
}

fn look_contrast() -> f32 {
    2.0
}

fn path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::sys::home().join(".local/share"));
    base.join("ytui/state.json")
}

impl Store {
    pub fn load() -> Store {
        std::fs::read(path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Store {
                autoplay: true,
                volume: 1.0,
                show_chat: true,
                exposure: true,
                look_bright: look_bright(),
                look_sat: look_sat(),
                look_bg: look_bg(),
                look_contrast: look_contrast(),
                ..Default::default()
            })
    }

    pub fn touch(&mut self) {
        self.dirty = true;
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        if let Ok(s) = serde_json::to_vec_pretty(self) {
            if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &p).is_ok() {
                self.dirty = false;
            }
        }
    }

    pub fn is_sub(&self, id: &str) -> bool {
        self.subs.iter().any(|c| c.id == id)
    }

    pub fn toggle_sub(&mut self, id: &str, name: &str) -> bool {
        self.dirty = true;
        if let Some(i) = self.subs.iter().position(|c| c.id == id) {
            self.subs.remove(i);
            false
        } else {
            self.subs.push(ChannelRef { id: id.into(), name: name.into(), sub_id: None });
            self.subs.sort_by_key(|c| c.name.to_lowercase());
            true
        }
    }

    pub fn sub_id(&self, channel: &str) -> Option<String> {
        self.subs.iter().find(|c| c.id == channel).and_then(|c| c.sub_id.clone())
    }

    pub fn set_sub_id(&mut self, channel: &str, sub_id: Option<String>) {
        if let Some(c) = self.subs.iter_mut().find(|c| c.id == channel) {
            c.sub_id = sub_id;
            self.dirty = true;
        }
    }

    /// Channels subscribed in ytui only (not on the YouTube account).
    pub fn local_only(&self) -> Vec<ChannelRef> {
        self.subs.iter().filter(|c| c.sub_id.is_none()).cloned().collect()
    }

    /// Make the account's subscriptions authoritative: add/refresh every
    /// remote one, drop ones that were on YouTube but no longer are, keep
    /// local-only ones. Returns (added, removed).
    pub fn merge_remote(&mut self, remote: &[(String, String, String)]) -> (usize, usize) {
        let ids: std::collections::HashSet<&str> = remote.iter().map(|r| r.0.as_str()).collect();
        let before = self.subs.len();
        self.subs.retain(|c| c.sub_id.is_none() || ids.contains(c.id.as_str()));
        let removed = before - self.subs.len();
        let mut added = 0;
        for (id, name, sub_id) in remote {
            match self.subs.iter_mut().find(|c| &c.id == id) {
                Some(c) => {
                    c.sub_id = Some(sub_id.clone());
                    if !name.is_empty() {
                        c.name = name.clone();
                    }
                }
                None => {
                    self.subs.push(ChannelRef { id: id.clone(), name: name.clone(), sub_id: Some(sub_id.clone()) });
                    added += 1;
                }
            }
        }
        self.subs.sort_by_key(|c| c.name.to_lowercase());
        self.dirty = true;
        (added, removed)
    }

    pub fn progress(&self, id: &str) -> Option<&HistEntry> {
        self.history.iter().find(|h| h.video.id == id)
    }

    /// Where to resume `id`, if it was left part-way through.
    pub fn resume_pos(&self, id: &str) -> f64 {
        match self.progress(id) {
            Some(h) if h.pos > 10.0 && (h.dur <= 0.0 || h.pos < h.dur - 15.0) => h.pos,
            _ => 0.0,
        }
    }

    pub fn record(&mut self, v: &VideoItem, pos: f64, dur: f64) {
        self.dirty = true;
        let mut e = match self.history.iter().position(|h| h.video.id == v.id) {
            Some(i) => self.history.remove(i),
            None => HistEntry { video: v.clone(), pos, dur, at: 0 },
        };
        if !v.title.is_empty() {
            e.video = v.clone();
        }
        e.pos = pos;
        e.dur = dur;
        e.at = now_secs();
        self.history.insert(0, e);
        self.history.truncate(2000);
    }

    pub fn toggle_later(&mut self, v: &VideoItem) -> bool {
        self.dirty = true;
        if let Some(i) = self.later.iter().position(|x| x.id == v.id) {
            self.later.remove(i);
            false
        } else {
            self.later.insert(0, v.clone());
            true
        }
    }

    /// Import a Google Takeout subscriptions.csv (Channel Id,Channel Url,Channel Title).
    pub fn import_csv(&mut self, csv: &str) -> usize {
        let mut n = 0;
        for line in csv.lines().skip(1) {
            let cols: Vec<&str> = line.splitn(3, ',').collect();
            if cols.len() == 3 && cols[0].starts_with("UC") && !self.is_sub(cols[0]) {
                self.subs.push(ChannelRef { id: cols[0].trim().into(), name: cols[2].trim().into(), sub_id: None });
                n += 1;
            }
        }
        self.subs.sort_by_key(|c| c.name.to_lowercase());
        self.dirty = true;
        n
    }
}
