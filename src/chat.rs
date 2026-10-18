// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

//! Live chat and chat replay. A background poller fetches messages and
//! hands them to the app; for replays it keeps ~20 s ahead of the player
//! position and restarts from the seek token when the user jumps around.

use crate::api::{Api, ChatMsg};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const KEEP: usize = 400;

pub enum Update {
    Msgs(Vec<ChatMsg>),
    /// Replay jumped: drop what we have.
    Reset,
    Error(String),
}

pub struct Chat {
    pub video: String,
    pub replay: bool,
    /// Live: newest last. Replay: everything fetched, ordered by offset.
    pub msgs: VecDeque<ChatMsg>,
    pub error: Option<String>,
    pos_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl Drop for Chat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Chat {
    /// `send` delivers updates tagged with the video id to the app loop.
    pub fn start(api: Arc<Api>, video: String, token: String, replay: bool, pos_ms: u64, send: impl Fn(String, Update) + Send + 'static) -> Chat {
        let stop = Arc::new(AtomicBool::new(false));
        let pos = Arc::new(AtomicU64::new(pos_ms));
        let (st, ps, vid) = (stop.clone(), pos.clone(), video.clone());
        std::thread::spawn(move || {
            let say = |u: Update| send(vid.clone(), u);
            if replay {
                replay_loop(&api, &token, &st, &ps, say);
            } else {
                live_loop(&api, token, &st, say);
            }
        });
        Chat { video, replay, msgs: VecDeque::new(), error: None, pos_ms: pos, stop }
    }

    pub fn set_pos(&self, secs: f64) {
        self.pos_ms.store((secs.max(0.0) * 1000.0) as u64, Ordering::Relaxed);
    }

    pub fn apply(&mut self, u: Update) {
        match u {
            Update::Msgs(m) => {
                self.error = None;
                self.msgs.extend(m);
                if self.replay {
                    self.msgs.make_contiguous().sort_by_key(|m| m.offset_ms.unwrap_or(0));
                    // Keep a window around the playhead.
                    let now = self.pos_ms.load(Ordering::Relaxed);
                    while self.msgs.len() > KEEP * 2 && self.msgs.front().is_some_and(|m| m.offset_ms.unwrap_or(0) + 120_000 < now) {
                        self.msgs.pop_front();
                    }
                } else {
                    while self.msgs.len() > KEEP {
                        self.msgs.pop_front();
                    }
                }
            }
            Update::Reset => self.msgs.clear(),
            Update::Error(e) => self.error = Some(e),
        }
    }

    /// Messages to show, newest last.
    pub fn visible(&self) -> impl DoubleEndedIterator<Item = &ChatMsg> {
        let now = self.pos_ms.load(Ordering::Relaxed);
        let replay = self.replay;
        self.msgs.iter().filter(move |m| !replay || m.offset_ms.unwrap_or(0) <= now)
    }
}

fn sleep_while(stop: &AtomicBool, ms: u64) -> bool {
    let mut left = ms;
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(100);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    !stop.load(Ordering::Relaxed)
}

fn live_loop(api: &Api, mut token: String, stop: &AtomicBool, say: impl Fn(Update)) {
    let mut fails = 0;
    while !stop.load(Ordering::Relaxed) {
        match api.live_chat(&token) {
            Ok(b) => {
                fails = 0;
                if !b.msgs.is_empty() {
                    say(Update::Msgs(b.msgs));
                }
                match b.next {
                    Some(t) => token = t,
                    None => {
                        say(Update::Error("chat ended".into()));
                        return;
                    }
                }
                if !sleep_while(stop, b.timeout_ms.clamp(1000, 3000)) {
                    return;
                }
            }
            Err(e) => {
                fails += 1;
                say(Update::Error(e.to_string()));
                if fails > 5 || !sleep_while(stop, 3000 * fails) {
                    return;
                }
            }
        }
    }
}

fn replay_loop(api: &Api, initial: &str, stop: &AtomicBool, pos: &AtomicU64, say: impl Fn(Update)) {
    let mut seek_tok = initial.to_string();
    let mut next: Option<String> = None;
    // Range of video time we have fetched: [from, to].
    let (mut from, mut to) = (u64::MAX, 0u64);
    let mut ended = false;
    while !stop.load(Ordering::Relaxed) {
        let now = pos.load(Ordering::Relaxed);
        let jumped = from != u64::MAX && (now + 2_000 < from || (!ended && now > to + 30_000));
        if jumped {
            ended = false;
        }
        let (tok, at) = if jumped || (next.is_none() && !ended) {
            if from != u64::MAX {
                say(Update::Reset);
            }
            from = now;
            to = now;
            (seek_tok.clone(), now)
        } else if !ended && now + 20_000 >= to {
            (next.clone().unwrap(), to)
        } else {
            if !sleep_while(stop, 500) {
                return;
            }
            continue;
        };
        match api.chat_replay(&tok, at) {
            Ok(b) => {
                let last = b.msgs.iter().filter_map(|m| m.offset_ms).max();
                if let Some(s) = b.seek {
                    seek_tok = s;
                }
                next = b.next;
                to = last.map_or(to + 30_000, |l| l.max(to + 1));
                if !b.msgs.is_empty() {
                    say(Update::Msgs(b.msgs));
                }
                // End of replay: idle until the user seeks back.
                ended = next.is_none();
            }
            Err(e) => {
                say(Update::Error(e.to_string()));
                if !sleep_while(stop, 3000) {
                    return;
                }
            }
        }
        if !sleep_while(stop, 300) {
            return;
        }
    }
}
