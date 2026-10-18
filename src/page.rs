//! Page model and layout. Every page is laid out into a `Doc`: positioned
//! primitives (text, images, the player) plus clickable / focusable targets,
//! in document coordinates. The app scrolls the doc, draws what is visible,
//! hit-tests the mouse against targets and moves keyboard focus spatially
//! between them, like a browser.

use crate::api::{ChannelPage, Comment, Entry, Feed, Item, PlaylistPage, VideoItem, WatchInfo};
use crate::auth::{MyPlaylist, PlItem};
use crate::music::{self, Browse, CardKind, Header, MItem, MusicPage, Song};
use crate::store::Store;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const ACCENT: Color = Color::Rgb(255, 60, 60);
pub const META: Color = Color::Rgb(170, 170, 170);
pub const DIM: Color = Color::Rgb(115, 115, 125);
pub const PANEL: Color = Color::Rgb(32, 32, 38);
pub const BUTTON: Color = Color::Rgb(52, 52, 62);

pub const CHIPS: &[&str] = &["For you", "Trending", "Music", "Gaming", "News", "Live", "Sports", "Learning", "Podcasts", "Movies"];
pub const FILTERS: &[(&str, Option<&str>)] = &[
    ("All", None),
    ("Videos", Some("EgIQAQ==")),
    ("Shorts", Some("EgIQCQ==")),
    ("Channels", Some("EgIQAg==")),
    ("Playlists", Some("EgIQAw==")),
    ("Live", Some("EgJAAQ==")),
    ("Newest", Some("CAI=")),
];

/// Query + filter behind a Home chip (other than "For you").
pub fn chip_query(chip: &str) -> (&'static str, Option<&'static str>) {
    match chip {
        "Trending" => ("trending now", Some("EgIQAQ==")),
        "Music" => ("music", Some("EgIQAQ==")),
        "Gaming" => ("gaming", Some("EgIQAQ==")),
        "News" => ("news today", Some("EgIQAQ==")),
        "Live" => ("live", Some("EgJAAQ==")),
        "Sports" => ("sports highlights", Some("EgIQAQ==")),
        "Learning" => ("learn something new", Some("EgIQAQ==")),
        "Podcasts" => ("podcast full episode", Some("EgIQAQ==")),
        _ => ("movie trailers", Some("EgIQAQ==")),
    }
}

pub struct Watch {
    pub video: VideoItem,
    pub info: Option<WatchInfo>,
    pub desc_open: bool,
    pub comments: Vec<Comment>,
    pub comments_cont: Option<String>,
    pub comment_count: Option<String>,
    pub comments_loading: bool,
    /// Playlist being played through, and our index in it.
    pub queue: Vec<VideoItem>,
    pub qpos: usize,
    /// Your rating when signed in: "like" / "dislike" / "none".
    pub rating: Option<String>,
    /// Playing from YouTube Music: the music player's state.
    pub music: Option<MusicNow>,
}

/// The YouTube Music player page: queue, lyrics and related.
#[derive(Default)]
pub struct MusicNow {
    /// Playlist the song plays in (None: a radio from the song).
    pub playlist: Option<String>,
    /// 0 Up next, 1 Lyrics, 2 Related.
    pub tab: usize,
    pub next: Option<music::Next>,
    pub lyrics: Option<Result<music::Lyrics, String>>,
    pub related: Option<Result<MusicPage, String>>,
    pub album: Option<(String, String)>,
    /// The list the song was played from (an album, playlist or shelf),
    /// kept as the queue; empty starts a radio from the song.
    pub list: Vec<Song>,
}

/// Which YouTube Music page.
#[derive(Clone, Debug, PartialEq)]
pub enum MRoute {
    Home,
    Explore,
    /// A library shelf (index into music::LIBRARY).
    Library(usize),
    Search { q: String, filter: usize },
    Browse(Browse),
}

pub enum Kind {
    Home { chip: usize, feed: Feed },
    Search { q: String, filter: usize, feed: Feed },
    Channel { id: String, params: Option<String>, page: Option<ChannelPage> },
    Playlist { id: String, page: Option<PlaylistPage> },
    Watch(Box<Watch>),
    Subs { feed: Feed },
    History,
    Later,
    Help,
    Account,
    /// Your playlists (signed in).
    Playlists { list: Option<Vec<MyPlaylist>> },
    /// One of your playlists, managed through the Data API.
    MyPlaylist { id: String, info: Option<MyPlaylist>, items: Vec<PlItem> },
    /// A YouTube Music page.
    Music { route: MRoute, page: Option<MusicPage> },
}

/// Per-draw context that isn't part of the page itself.
pub struct Ctx {
    pub autoplay: bool,
    pub chat: bool,
    /// Signed-in channel name, if signed in.
    pub account: Option<String>,
    /// Signed-in YouTube account (browser sign-in), if known.
    pub yt_account: Option<String>,
    /// Saved YouTube sign-in: (cookie count, age), if any.
    pub yt_cookies: Option<(usize, std::time::Duration)>,
    /// Waiting for a cookie extension's export on the clipboard.
    pub capturing: bool,
    /// A Google sign-in window is open.
    pub logging_in: bool,
}

pub struct Page {
    pub uid: u64,
    pub kind: Kind,
    pub scroll: usize,
    pub focus: Option<usize>,
    pub loading: bool,
    pub more_loading: bool,
    pub error: Option<String>,
    pub rev: u64,
    pub layout: Option<((u16, u16, u64, u64), Doc)>,
}

impl Page {
    pub fn new(uid: u64, kind: Kind) -> Page {
        Page { uid, kind, scroll: 0, focus: None, loading: false, more_loading: false, error: None, rev: 0, layout: None }
    }

    pub fn changed(&mut self) {
        self.rev += 1;
    }

    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Home { .. } => "Home".into(),
            Kind::Search { q, .. } => q.clone(),
            Kind::Channel { page, .. } => page.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Channel".into()),
            Kind::Playlist { page, .. } => page.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| "Playlist".into()),
            Kind::Watch(w) if w.music.is_some() && !w.video.title.is_empty() => w.video.title.clone(),
            Kind::Watch(w) => w.info.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| w.video.title.clone()),
            Kind::Subs { .. } => "Subscriptions".into(),
            Kind::History => "History".into(),
            Kind::Later => "Watch later".into(),
            Kind::Help => "Help".into(),
            Kind::Account => "Account".into(),
            Kind::Playlists { .. } => "Playlists".into(),
            Kind::MyPlaylist { info, .. } => info.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| "Playlist".into()),
            Kind::Music { route, page } => match (route, page.as_ref().and_then(|p| p.header.as_ref())) {
                (MRoute::Search { q, .. }, _) => q.clone(),
                (_, Some(h)) => h.title.clone(),
                (MRoute::Home, _) => "YouTube Music".into(),
                (MRoute::Explore, _) => "Explore".into(),
                (MRoute::Library(i), _) => music::LIBRARY[*i].0.into(),
                (MRoute::Browse(_), _) => "YouTube Music".into(),
            },
        }
    }

    pub fn address(&self) -> String {
        match &self.kind {
            Kind::Home { .. } => "ytui://home".into(),
            Kind::Search { q, .. } => format!("youtube.com/results?search_query={}", crate::api::urlencode(q)),
            Kind::Channel { id, .. } => format!("youtube.com/channel/{id}"),
            Kind::Playlist { id, .. } => format!("youtube.com/playlist?list={id}"),
            Kind::Watch(w) if w.music.is_some() => format!("music.youtube.com/watch?v={}", w.video.id),
            Kind::Watch(w) => format!("youtube.com/watch?v={}", w.video.id),
            Kind::Subs { .. } => "ytui://subscriptions".into(),
            Kind::History => "ytui://history".into(),
            Kind::Later => "ytui://watch-later".into(),
            Kind::Help => "ytui://help".into(),
            Kind::Account => "ytui://account".into(),
            Kind::Playlists { .. } => "ytui://playlists".into(),
            Kind::MyPlaylist { id, .. } => format!("youtube.com/playlist?list={id}"),
            Kind::Music { route, .. } => match route {
                MRoute::Home => "music.youtube.com".into(),
                MRoute::Explore => "music.youtube.com/explore".into(),
                MRoute::Library(_) => "music.youtube.com/library".into(),
                MRoute::Search { q, .. } => format!("music.youtube.com/search?q={}", crate::api::urlencode(q)),
                MRoute::Browse(b) => music::address(b),
            },
        }
    }

    /// Continuation token for infinite scroll, if any.
    pub fn more_token(&self) -> Option<&str> {
        match &self.kind {
            Kind::Home { feed, .. } | Kind::Search { feed, .. } => feed.cont.as_deref(),
            Kind::Channel { page: Some(p), .. } => p.feed.cont.as_deref(),
            Kind::Playlist { page: Some(p), .. } => p.feed.cont.as_deref(),
            Kind::Watch(w) => w.comments_cont.as_deref(),
            Kind::Music { page: Some(p), .. } => p.cont.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Act {
    Video(VideoItem),
    /// Open video `idx` of a list, playing through the rest as a queue.
    QueueVideo(Vec<VideoItem>, usize),
    Channel(String),
    Playlist(String),
    Chip(usize),
    Filter(usize),
    ChannelTab(usize),
    ToggleSub(String, String),
    ToggleLater(VideoItem),
    /// Open / cancel the Google sign-in window.
    SignIn,
    /// Start / cancel picking up cookies from a cookie extension.
    SignInCookies,
    SignOutVideos,
    ExpandDesc,
    Replies(usize),
    Seek(f64),
    Player,
    PlayerBar,
    PlayAll,
    ClearHistory,
    /// Like / dislike (toggles back to none).
    Rate(String),
    /// Open the composer for a new comment.
    Compose,
    /// Open the composer to reply to comment `idx`.
    ReplyTo(usize),
    SignOut,
    /// Re-read subscriptions from the YouTube account.
    SyncSubs,
    /// Subscribe on YouTube to every local-only channel.
    UploadSubs,
    /// Open the composer to send a live chat message.
    ChatSend,
    /// "Save to…" dialog for a video.
    SaveTo(VideoItem),
    NewPlaylist,
    OpenMyPlaylist(String),
    PlPlayAll,
    PlRename,
    PlPrivacy,
    PlDelete,
    /// Move item idx by -1 / +1.
    PlMove(usize, i64),
    PlRemove(usize),
    /// Go to a YouTube Music page.
    MusicGo(MRoute),
    /// Item (section, index) of the music page shown: play a song in its
    /// list, or open a card.
    MusicItem(usize, usize),
    /// Same, for the Related tab of the music player.
    MusicRelated(usize, usize),
    /// Play entry n of the music player's queue.
    MusicQueue(usize),
    /// A song's ⋮ menu (Play next, Add to queue, Go to album…).
    SongMenu(Box<Song>),
    /// Play a playlist from its start (a header's Play / Shuffle / Radio).
    MusicPlaylist(String),
    /// Search filter / library shelf / player tab.
    MusicFilter(usize),
    MusicLib(usize),
    MusicTab(usize),
    /// Open the address bar to search YouTube Music.
    MusicSearch,
}

#[derive(Clone)]
pub enum Prim {
    Text { x: u16, y: usize, line: Line<'static> },
    Image { x: u16, y: usize, w: u16, h: u16, url: String, badge: Option<(String, Color)>, progress: Option<f32> },
    Fill { x: u16, y: usize, w: u16, h: u16, color: Color },
    Player { x: u16, y: usize, w: u16, h: u16, id: String },
    PlayerBar { x: u16, y: usize, w: u16, id: String },
    /// Live chat / chat replay panel, filled in at draw time. `input`
    /// reserves the bottom row for the send box (live chats only).
    Chat { x: u16, y: usize, w: u16, h: u16, input: bool },
}

#[derive(Clone)]
pub struct Tgt {
    pub x: u16,
    pub y: usize,
    pub w: u16,
    pub h: u16,
    pub act: Act,
}

#[derive(Clone, Default)]
pub struct Doc {
    pub h: usize,
    pub prims: Vec<Prim>,
    pub targets: Vec<Tgt>,
}

/// Greedy word wrap to display width `w`, at most `max` lines ("…" if cut).
pub fn wrap(s: &str, w: usize, max: usize) -> Vec<String> {
    let w = w.max(2);
    let s = s.replace('\t', "    ").replace('\r', "");
    let mut lines: Vec<String> = Vec::new();
    let mut cut = false;
    'outer: for para in s.split('\n') {
        let mut cur = String::new();
        let mut cw = 0;
        for word in para.split(' ') {
            let ww = word.width();
            if cw > 0 && cw + 1 + ww > w {
                lines.push(std::mem::take(&mut cur));
                cw = 0;
                if lines.len() >= max {
                    cut = true;
                    break 'outer;
                }
            }
            if ww > w {
                for ch in word.chars() {
                    let c = ch.width().unwrap_or(0);
                    if cw + c > w {
                        lines.push(std::mem::take(&mut cur));
                        cw = 0;
                        if lines.len() >= max {
                            cut = true;
                            break 'outer;
                        }
                    }
                    cur.push(ch);
                    cw += c;
                }
                continue;
            }
            if cw > 0 {
                cur.push(' ');
                cw += 1;
            }
            cur.push_str(word);
            cw += ww;
        }
        lines.push(cur);
        if lines.len() >= max {
            cut = true;
            break;
        }
    }
    if cut && lines.len() >= max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            while last.width() > w - 1 {
                last.pop();
            }
            last.push('…');
        }
    }
    lines
}

pub fn trunc(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if out.width() + ch.width().unwrap_or(0) + 1 > w {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Parse "1:02:03" / "4:05" at the start of a description line.
pub fn timestamp(line: &str) -> Option<f64> {
    let tok = line.split_whitespace().next()?;
    let tok = tok.trim_start_matches(['(', '[']).trim_end_matches([')', ']', '-', ':', '–']);
    let parts: Vec<&str> = tok.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p.parse::<f64>().unwrap_or(0.0)))
}

struct B<'a> {
    prims: Vec<Prim>,
    tg: Vec<Tgt>,
    store: &'a Store,
}

fn st(c: Color) -> Style {
    Style::default().fg(c)
}

impl B<'_> {
    fn line(&mut self, x: u16, y: usize, spans: Vec<Span<'static>>) {
        self.prims.push(Prim::Text { x, y, line: Line::from(spans) });
    }

    fn para(&mut self, x: u16, y: usize, w: u16, text: &str, style: Style, max: usize) -> usize {
        let lines = wrap(text, w as usize, max);
        let n = lines.len();
        for (i, l) in lines.into_iter().enumerate() {
            self.line(x, y + i, vec![Span::styled(l, style)]);
        }
        n
    }

    fn tgt(&mut self, x: u16, y: usize, w: u16, h: u16, act: Act) {
        self.tg.push(Tgt { x, y, w: w.max(1), h: h.max(1), act });
    }

    fn button(&mut self, x: u16, y: usize, label: &str, act: Act, on: bool) -> u16 {
        let s = format!(" {label} ");
        let w = s.width() as u16;
        let style = if on { Style::default().fg(Color::Black).bg(Color::Rgb(235, 235, 235)) } else { Style::default().fg(Color::White).bg(BUTTON) };
        self.line(x, y, vec![Span::styled(s, style)]);
        self.tgt(x, y, w, 1, act);
        w
    }

    /// A row of chip buttons that wraps; returns rows used.
    fn chips(&mut self, x0: u16, y: usize, w: u16, labels: &[String], sel: Option<usize>, act: impl Fn(usize) -> Act) -> usize {
        let (mut x, mut row) = (x0, 0);
        for (i, l) in labels.iter().enumerate() {
            let lw = l.width() as u16 + 2;
            if x > x0 && x + lw > x0 + w {
                x = x0;
                row += 2;
            }
            x += self.button(x, y + row, l, act(i), sel == Some(i)) + 1;
        }
        row + 1
    }

    fn video_badge(v: &VideoItem) -> Option<(String, Color)> {
        if v.live {
            Some((" LIVE ".into(), Color::Rgb(204, 0, 0)))
        } else if v.short {
            Some((" SHORTS ".into(), Color::Rgb(204, 0, 0)))
        } else {
            v.duration.as_ref().map(|d| (format!(" {d} "), Color::Black))
        }
    }

    fn progress(&self, id: &str) -> Option<f32> {
        self.store.progress(id).filter(|h| h.dur > 0.0 && h.pos > 5.0).map(|h| (h.pos / h.dur).clamp(0.0, 1.0) as f32)
    }

    fn thumb(&mut self, x: u16, y: usize, w: u16, h: u16, item: &Item) {
        let (url, badge, progress) = match item {
            Item::Video(v) => (v.thumb(), Self::video_badge(v), self.progress(&v.id)),
            Item::Playlist(p) => (
                p.thumb.clone().or_else(|| p.first_video.as_ref().map(|v| format!("https://i.ytimg.com/vi/{v}/mqdefault.jpg"))).unwrap_or_default(),
                Some((format!(" ▤ {} ", p.count.clone().unwrap_or_else(|| "Playlist".into())), Color::Black)),
                None,
            ),
            Item::Channel(c) => (c.thumb.clone().unwrap_or_default(), None, None),
        };
        if !url.is_empty() {
            self.prims.push(Prim::Image { x, y, w, h, url, badge, progress });
        }
    }

    fn item_act(item: &Item, list: &[VideoItem]) -> Act {
        match item {
            Item::Video(v) => match list.iter().position(|x| x.id == v.id) {
                Some(i) if list.len() > 1 => Act::QueueVideo(list.to_vec(), i),
                _ => Act::Video(v.clone()),
            },
            Item::Channel(c) => Act::Channel(c.id.clone()),
            Item::Playlist(p) => match (&p.first_video, p.id.starts_with("RD")) {
                (Some(v), true) => Act::Video(VideoItem { id: v.clone(), title: p.title.clone(), ..Default::default() }),
                _ => Act::Playlist(p.id.clone()),
            },
        }
    }

    fn item_lines(item: &Item) -> (String, Vec<String>) {
        match item {
            Item::Video(v) => {
                let mut l2 = Vec::new();
                if let Some(vw) = &v.views {
                    l2.push(vw.clone());
                }
                if let Some(p) = &v.published {
                    l2.push(p.clone());
                }
                let mut meta = Vec::new();
                if !v.channel.is_empty() {
                    meta.push(v.channel.clone());
                }
                if !l2.is_empty() {
                    meta.push(l2.join(" · "));
                }
                (v.title.clone(), meta)
            }
            Item::Channel(c) => {
                let mut l = Vec::new();
                if let Some(h) = &c.handle {
                    l.push(h.clone());
                }
                if let Some(s) = &c.subs {
                    l.push(s.clone());
                }
                (c.name.clone(), vec![l.join(" · "), "Channel".into()])
            }
            Item::Playlist(p) => {
                let mut meta = Vec::new();
                if let Some(c) = &p.channel {
                    meta.push(c.clone());
                }
                meta.push("View full playlist".into());
                (p.title.clone(), meta)
            }
        }
    }

    /// YouTube-home style card: thumbnail above, text below.
    fn grid_card(&mut self, x: u16, y: usize, w: u16, item: &Item, list: &[VideoItem]) -> usize {
        let th = ((w as f32) * 0.28125).round().max(3.0) as u16;
        let (tx, tw) = match item {
            Item::Channel(_) => {
                let cw = (th * 2).min(w);
                (x + (w - cw) / 2, cw)
            }
            _ => (x, w),
        };
        self.thumb(tx, y, tw, th, item);
        let (title, meta) = Self::item_lines(item);
        let mut yy = y + th as usize + 1;
        yy += self.para(x, yy, w, &title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD), 2);
        for m in meta.iter().filter(|m| !m.is_empty()) {
            self.line(x, yy, vec![Span::styled(trunc(m, w as usize), st(META))]);
            yy += 1;
        }
        let h = yy - y;
        self.tgt(x, y, w, h as u16, Self::item_act(item, list));
        h
    }

    /// Search-results style card: thumbnail left, text right.
    fn list_card(&mut self, x: u16, y: usize, w: u16, tw: u16, item: &Item, list: &[VideoItem], prefix: Option<String>) -> usize {
        let th = ((tw as f32) * 0.28125).round().max(2.0) as u16;
        let (ix, iw) = match item {
            Item::Channel(_) => (x + tw / 4, (th * 2).min(tw)),
            _ => (x, tw),
        };
        self.thumb(ix, y, iw, th, item);
        let tx = x + tw + 2;
        let ww = w.saturating_sub(tw + 2).max(8);
        let (mut title, meta) = Self::item_lines(item);
        if let Some(p) = prefix {
            title = format!("{p}{title}");
        }
        let mut yy = y;
        yy += self.para(tx, yy, ww, &title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD), 2);
        for m in meta.iter().filter(|m| !m.is_empty()) {
            if yy >= y + th as usize {
                break;
            }
            self.line(tx, yy, vec![Span::styled(trunc(m, ww as usize), st(META))]);
            yy += 1;
        }
        let h = (th as usize).max(yy - y);
        self.tgt(x, y, w, h as u16, Self::item_act(item, list));
        h
    }

    fn grid(&mut self, y0: usize, w: u16, entries: &[Entry]) -> usize {
        let list: Vec<VideoItem> = Vec::new();
        let n = ((w + 2) / 36).max(1);
        let cw = (w + 2) / n - 2;
        let mut y = y0;
        let mut col = 0;
        let mut row_h = 0;
        for e in entries {
            match e {
                Entry::Heading(t) => {
                    if col > 0 {
                        y += row_h + 1;
                        col = 0;
                        row_h = 0;
                    }
                    self.line(1, y, vec![Span::styled(t.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]);
                    y += 2;
                }
                Entry::Item(it) => {
                    let x = 1 + col * (cw + 2);
                    row_h = row_h.max(self.grid_card(x, y, cw, it, &list));
                    col += 1;
                    if col == n {
                        y += row_h + 1;
                        col = 0;
                        row_h = 0;
                    }
                }
            }
        }
        if col > 0 {
            y += row_h + 1;
        }
        y
    }

    fn list(&mut self, y0: usize, w: u16, entries: &[Entry], numbered: bool) -> usize {
        let tw = (w * 35 / 100).clamp(16, 44);
        let videos: Vec<VideoItem> = if numbered {
            entries.iter().filter_map(|e| if let Entry::Item(Item::Video(v)) = e { Some(v.clone()) } else { None }).collect()
        } else {
            Vec::new()
        };
        let mut y = y0;
        let mut n = 0;
        for e in entries {
            match e {
                Entry::Heading(t) => {
                    y += 1;
                    self.line(1, y, vec![Span::styled(t.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]);
                    y += 2;
                }
                Entry::Item(it) => {
                    n += 1;
                    let prefix = numbered.then(|| format!("{n}. "));
                    y += self.list_card(1, y, w - 1, tw, it, &videos, prefix) + 1;
                }
            }
        }
        y
    }

    fn status(&mut self, y: usize, page: &Page, empty: &str, has_items: bool) -> usize {
        let msg = if let Some(e) = &page.error {
            Some((format!("⚠ {e}"), ACCENT))
        } else if page.loading {
            Some(("Loading…".to_string(), META))
        } else if !has_items {
            Some((empty.to_string(), META))
        } else {
            None
        };
        match msg {
            Some((m, c)) => {
                self.line(1, y + 1, vec![Span::styled(m, st(c))]);
                y + 3
            }
            None => y,
        }
    }
}

/// Lay out `page` for a content area `w` wide and `vh` tall.
pub fn layout(page: &Page, store: &Store, w: u16, vh: u16, ctx: &Ctx) -> Doc {
    let mut b = B { prims: Vec::new(), tg: Vec::new(), store };
    let w = w.max(20);
    let iw = w - 1; // inner width after the 1-col focus gutter
    let bold = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let mut y = 0usize;
    match &page.kind {
        Kind::Home { chip, feed } => {
            let labels: Vec<String> = CHIPS.iter().map(|s| s.to_string()).collect();
            y += b.chips(1, y, iw, &labels, Some(*chip), Act::Chip) + 1;
            if *chip == 0 && feed.entries.is_empty() && !page.loading && page.error.is_none() {
                for (i, l) in [
                    "Nothing to recommend yet.",
                    "",
                    "Watch a few videos or subscribe to channels and \"For you\" fills up with",
                    "recommendations based on what you watch, plus new uploads from your",
                    "subscriptions. Meanwhile try a chip above, or press / to search.",
                ]
                .iter()
                .enumerate()
                {
                    b.line(1, y + 1 + i, vec![Span::styled(l.to_string(), st(if i == 0 { Color::White } else { META }))]);
                }
                y += 7;
            } else {
                y = b.status(y, page, "No videos.", !feed.entries.is_empty());
                y = b.grid(y, iw, &feed.entries);
            }
        }
        Kind::Search { filter, feed, .. } => {
            let labels: Vec<String> = FILTERS.iter().map(|f| f.0.to_string()).collect();
            y += b.chips(1, y, iw, &labels, Some(*filter), Act::Filter) + 1;
            y = b.status(y, page, "No results.", !feed.entries.is_empty());
            y = b.list(y, iw, &feed.entries, false);
        }
        Kind::Channel { id, page: cp, .. } => {
            if let Some(c) = cp {
                if let Some(url) = &c.banner {
                    let bh = ((w as f32) * 0.07).clamp(3.0, 7.0) as u16;
                    b.prims.push(Prim::Image { x: 1, y, w: iw, h: bh, url: url.clone(), badge: None, progress: None });
                    y += bh as usize + 1;
                }
                let ah = 5u16;
                if let Some(a) = &c.avatar {
                    b.prims.push(Prim::Image { x: 1, y, w: ah * 2, h: ah, url: a.clone(), badge: None, progress: None });
                }
                let tx = 1 + ah * 2 + 2;
                let tw = iw.saturating_sub(tx);
                b.line(tx, y, vec![Span::styled(c.name.clone(), bold)]);
                b.line(tx, y + 1, vec![Span::styled(trunc(&c.meta.join(" · "), tw as usize), st(META))]);
                b.para(tx, y + 2, tw, &c.description, st(DIM), 2);
                let subbed = store.is_sub(id);
                b.button(tx, y + 4, if subbed { "Subscribed ✓" } else { "Subscribe" }, Act::ToggleSub(id.clone(), c.name.clone()), !subbed);
                y += ah as usize + 1;
                let (mut x, ty) = (1u16, y);
                for (i, (t, _)) in c.tabs.iter().enumerate() {
                    let sel = i == c.selected;
                    let label = format!(" {t} ");
                    let lw = label.width() as u16;
                    let style = if sel { Style::default().fg(Color::White).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { st(META) };
                    b.line(x, ty, vec![Span::styled(label, style)]);
                    b.tgt(x, ty, lw, 1, Act::ChannelTab(i));
                    x += lw + 2;
                }
                b.line(1, ty + 1, vec![Span::styled("─".repeat(iw as usize), st(Color::Rgb(60, 60, 70)))]);
                y = ty + 3;
                y = b.status(y, page, "This tab is empty.", !c.feed.entries.is_empty());
                y = b.grid(y, iw, &c.feed.entries);
            } else {
                y = b.status(y, page, "", false);
            }
        }
        Kind::Playlist { page: pp, .. } => {
            if let Some(p) = pp {
                y += b.para(1, y, iw, &p.title, bold, 2);
                b.line(1, y, vec![Span::styled(trunc(&p.meta.join(" · "), iw as usize), st(META))]);
                y += 2;
                let x = b.button(1, y, "▶ Play all", Act::PlayAll, true);
                let _ = x;
                y += 2;
                y = b.status(y, page, "This playlist is empty.", !p.feed.entries.is_empty());
                y = b.list(y, iw, &p.feed.entries, true);
            } else {
                y = b.status(y, page, "", false);
            }
        }
        Kind::Watch(wt) => {
            y = match &wt.music {
                Some(m) => music_watch_layout(&mut b, wt, m, w, vh),
                None => watch_layout(&mut b, page, wt, w, vh, ctx),
            }
        }
        Kind::Music { route, page: mp } => y = music_layout(&mut b, page, route, mp.as_ref(), iw, ctx),
        Kind::Subs { feed } => {
            b.line(1, y, vec![Span::styled("Subscriptions", bold)]);
            y += 2;
            if store.subs.is_empty() {
                for (i, l) in [
                    "You haven't subscribed to anything yet.",
                    "Press s on a video or channel page to subscribe. Subscriptions are kept",
                    "locally; import a Google Takeout subscriptions.csv with --import-subs FILE.",
                ]
                .iter()
                .enumerate()
                {
                    b.line(1, y + i, vec![Span::styled(l.to_string(), st(META))]);
                }
                y += 4;
            } else {
                if ctx.account.is_some() {
                    let local = store.local_only().len();
                    b.line(1, y, vec![Span::styled(format!("Synced with your YouTube account · {} channels", store.subs.len() - local), st(META))]);
                    let mut bx = 1;
                    y += 1;
                    bx += b.button(bx, y, "↻ Sync now", Act::SyncSubs, false) + 1;
                    if local > 0 {
                        b.button(bx, y, &format!("Upload {local} local-only to YouTube"), Act::UploadSubs, false);
                    }
                    y += 2;
                }
                let labels: Vec<String> = store.subs.iter().map(|c| if ctx.account.is_some() && c.sub_id.is_none() { format!("{} (local)", c.name) } else { c.name.clone() }).collect();
                let ids: Vec<String> = store.subs.iter().map(|c| c.id.clone()).collect();
                y += b.chips(1, y, iw, &labels, None, |i| Act::Channel(ids[i].clone())) + 1;
                b.line(1, y, vec![Span::styled("Latest", bold)]);
                y += 2;
                y = b.status(y, page, "No recent uploads.", !feed.entries.is_empty());
                y = b.grid(y, iw, &feed.entries);
            }
        }
        Kind::History => {
            b.line(1, y, vec![Span::styled("Watch history", bold)]);
            if !store.history.is_empty() {
                b.button(iw.saturating_sub(16), y, "Clear history", Act::ClearHistory, false);
            }
            y += 2;
            let now = crate::api::now_secs();
            let entries: Vec<Entry> = store
                .history
                .iter()
                .map(|h| {
                    let mut v = h.video.clone();
                    v.published = Some(format!("watched {}", crate::api::ago(now.saturating_sub(h.at))));
                    Entry::Item(Item::Video(v))
                })
                .collect();
            if entries.is_empty() {
                b.line(1, y, vec![Span::styled("Videos you watch show up here.", st(META))]);
                y += 2;
            }
            y = b.list(y, iw, &entries, false);
        }
        Kind::Later => {
            b.line(1, y, vec![Span::styled("Watch later", bold)]);
            y += 2;
            if store.later.is_empty() {
                b.line(1, y, vec![Span::styled("Press w on any video to save it here.", st(META))]);
                y += 2;
            }
            let entries: Vec<Entry> = store.later.iter().map(|v| Entry::Item(Item::Video(v.clone()))).collect();
            y = b.list(y, iw, &entries, true);
        }
        Kind::Playlists { list } => {
            b.line(1, y, vec![Span::styled("Your playlists", bold)]);
            if ctx.account.is_some() {
                b.button(iw.saturating_sub(16), y, "+ New playlist", Act::NewPlaylist, true);
            }
            y += 2;
            if ctx.account.is_none() && ctx.yt_account.is_none() {
                b.line(1, y, vec![Span::styled("Sign in to see your YouTube playlists (see the Account page).".to_string(), st(META))]);
                y += 2;
            } else if let Some(e) = &page.error {
                b.line(1, y, vec![Span::styled(format!("⚠ {e}"), st(ACCENT))]);
                y += 2;
            } else if page.loading || list.is_none() {
                b.line(1, y, vec![Span::styled("Loading…".to_string(), st(META))]);
                y += 2;
            } else if list.as_ref().is_some_and(|l| l.is_empty()) {
                b.line(1, y, vec![Span::styled("No playlists yet. Create one, or use ≡+ Save on any video.".to_string(), st(META))]);
                y += 2;
            }
            let tw = (iw * 30 / 100).clamp(16, 36);
            let th = ((tw as f32) * 0.28125).round() as u16;
            for p in list.iter().flatten() {
                match &p.thumb {
                    Some(u) => b.prims.push(Prim::Image { x: 1, y, w: tw, h: th, url: u.clone(), badge: Some((format!(" ▤ {} ", p.count), Color::Black)), progress: None }),
                    None => b.prims.push(Prim::Fill { x: 1, y, w: tw, h: th, color: PANEL }),
                }
                let tx = 1 + tw + 2;
                let ww = iw.saturating_sub(tw + 2);
                b.para(tx, y, ww, &p.title, bold, 2);
                b.line(tx, y + 2, vec![Span::styled(format!("{} videos · {}", p.count, p.privacy), st(META))]);
                // Editing needs the API sign-in; otherwise open it like any
                // playlist (as the signed-in user, so private ones work too).
                let act = if ctx.account.is_some() { Act::OpenMyPlaylist(p.id.clone()) } else { Act::Playlist(p.id.clone()) };
                b.tgt(1, y, iw, th, act);
                y += th as usize + 1;
            }
        }
        Kind::MyPlaylist { info, items, .. } => {
            match info {
                Some(i) => {
                    y += b.para(1, y, iw, &i.title, bold, 2);
                    b.line(1, y, vec![Span::styled(format!("{} videos · {} · your playlist", i.count, i.privacy), st(META))]);
                    y += 2;
                    let mut bx = 1;
                    bx += b.button(bx, y, "▶ Play all", Act::PlPlayAll, true) + 1;
                    bx += b.button(bx, y, "Rename", Act::PlRename, false) + 1;
                    bx += b.button(bx, y, &format!("Privacy: {}", i.privacy), Act::PlPrivacy, false) + 1;
                    b.button(bx, y, "Delete playlist", Act::PlDelete, false);
                    y += 2;
                }
                None => {}
            }
            y = b.status(y, page, "This playlist is empty.", !items.is_empty());
            let tw = (iw * 35 / 100).clamp(16, 44);
            let vids: Vec<VideoItem> = items.iter().map(|i| i.video.clone()).collect();
            for (idx, it) in items.iter().enumerate() {
                let h = b.list_card(1, y, iw - 1, tw, &Item::Video(it.video.clone()), &vids, Some(format!("{}. ", idx + 1))).max(3);
                // Leave the card's last row to the item buttons.
                if let Some(t) = b.tg.last_mut() {
                    t.h = (h as u16 - 1).max(1);
                }
                let by = y + h - 1;
                let mut bx = 1 + tw + 2;
                if idx > 0 {
                    bx += b.button(bx, by, "↑", Act::PlMove(idx, -1), false) + 1;
                }
                if idx + 1 < items.len() {
                    bx += b.button(bx, by, "↓", Act::PlMove(idx, 1), false) + 1;
                }
                b.button(bx, by, "✕ Remove", Act::PlRemove(idx), false);
                y += h + 1;
            }
        }
        Kind::Account => {
            b.line(1, y, vec![Span::styled("YouTube sign-in", bold)]);
            y += 2;
            if ctx.logging_in {
                for l in [
                    "A browser window is open on Google's sign-in page.",
                    "Sign in there as you would on any website. When you're done and YouTube",
                    "shows, close that window: ytui then saves your sign-in.",
                ] {
                    b.line(1, y, vec![Span::styled(l.to_string(), st(Color::White))]);
                    y += 1;
                }
                y += 1;
                b.button(1, y, "Cancel", Act::SignIn, false);
                y += 2;
            } else if ctx.capturing {
                for l in [
                    "Waiting for your cookies…",
                    "",
                    "In your browser, on youtube.com (signed in), click your cookie extension",
                    "(e.g. Cookie-Editor or Get cookies.txt LOCALLY) and press Export or Copy.",
                    "ytui picks them up from the clipboard by itself. Pasting here (Ctrl-V) works too.",
                ] {
                    b.line(1, y, vec![Span::styled(l.to_string(), st(if l.starts_with("Waiting") { Color::White } else { META }))]);
                    y += 1;
                }
                y += 1;
                b.button(1, y, "Cancel", Act::SignInCookies, false);
                y += 2;
            } else if let Some((n, age)) = ctx.yt_cookies {
                let days = age.as_secs() / 86400;
                let when = if days == 0 { "today".to_string() } else { format!("{days} day{} ago", if days == 1 { "" } else { "s" }) };
                let who = ctx.yt_account.clone().map(|a| format!("Signed in as {a} ✓")).unwrap_or_else(|| "Signed in ✓".to_string());
                b.line(1, y, vec![Span::styled(format!("{who}  (since {when}, {n} cookies)"), st(Color::Rgb(120, 220, 120)))]);
                y += 1;
                b.line(1, y, vec![Span::styled("Home shows your recommendations, Subscriptions your subscriptions feed, and".to_string(), st(META))]);
                y += 1;
                b.line(1, y, vec![Span::styled("videos play even when YouTube asks to confirm you're not a bot.".to_string(), st(META))]);
                y += 2;
                let w = b.button(1, y, "Sign in again", Act::SignIn, false);
                b.button(1 + w + 6, y, "Sign out", Act::SignOutVideos, false);
                y += 2;
            } else {
                for l in [
                    "Sign in with your Google account, like on any website: a sign-in window opens,",
                    "you sign in, and it closes by itself. Lets videos play when YouTube asks to",
                    "confirm you're not a bot. ytui never sees your password.",
                ] {
                    b.line(1, y, vec![Span::styled(l.to_string(), st(META))]);
                    y += 1;
                }
                y += 1;
                b.button(1, y, "Sign in with Google", Act::SignIn, true);
                y += 2;
                b.button(1, y, "Use my cookie extension instead", Act::SignInCookies, false);
                y += 2;
            }
            y += 1;
            match &ctx.account {
                Some(name) => {
                    b.line(1, y, vec![Span::styled("Account", bold)]);
                    y += 2;
                    let who = if name.is_empty() { "a Google account without a YouTube channel".to_string() } else { name.clone() };
                    b.line(1, y, vec![Span::styled("Signed in as ", st(META)), Span::styled(who, bold)]);
                    y += 2;
                    for l in [
                        "You can like and dislike videos, comment, and reply to comments.",
                        "Each like or comment uses 50 of your 10,000 daily API units (about 200 a day).",
                        "Subscriptions, history and Watch later stay local to ytui.",
                    ] {
                        b.line(1, y, vec![Span::styled(l.to_string(), st(META))]);
                        y += 1;
                    }
                    y += 1;
                    b.button(1, y, "Sign out", Act::SignOut, false);
                    y += 2;
                }
                None => {
                    b.line(1, y, vec![Span::styled("Likes and comments (optional, needs a Google Cloud OAuth client)", bold)]);
                    y += 2;
                    for l in crate::auth::SETUP_HELP.lines() {
                        b.line(1, y, vec![Span::styled(l.to_string(), st(if l.starts_with(' ') || l.is_empty() { META } else { Color::White }))]);
                        y += 1;
                    }
                    y += 1;
                    b.line(1, y, vec![Span::styled("Then restart ytui.", st(Color::White))]);
                    y += 2;
                }
            }
        }
        Kind::Help => {
            for (i, l) in HELP_TEXT.lines().enumerate() {
                let style = if !l.starts_with(' ') && !l.is_empty() { bold } else { st(META) };
                b.line(1, y + i, vec![Span::styled(l.to_string(), style)]);
            }
            y += HELP_TEXT.lines().count() + 1;
        }
    }
    Doc { h: y + 1, prims: b.prims, targets: b.tg }
}

fn watch_layout(b: &mut B, page: &Page, wt: &Watch, w: u16, vh: u16, ctx: &Ctx) -> usize {
    let (autoplay, chat) = (ctx.autoplay, ctx.chat);
    let bold = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let two = w >= 120;
    let lw = if two { w * 64 / 100 } else { w } - 1;
    // Player: 16:9 of the column, capped so the info peeks out below it.
    let max_h = ((vh as f32) * 0.66) as u16;
    let mut ph = ((lw as f32) * 0.28125).round() as u16;
    let mut pw = lw;
    if ph > max_h {
        ph = max_h.max(4);
        pw = ((ph as f32) / 0.28125).round() as u16;
    }
    let px = 1 + (lw - pw.min(lw)) / 2;
    b.prims.push(Prim::Player { x: px, y: 0, w: pw.min(lw), h: ph, id: wt.video.id.clone() });
    b.tgt(px, 0, pw.min(lw), ph, Act::Player);
    b.prims.push(Prim::PlayerBar { x: 1, y: ph as usize, w: lw, id: wt.video.id.clone() });
    b.tgt(1, ph as usize, lw, 1, Act::PlayerBar);
    let mut y = ph as usize + 2;
    let info = wt.info.as_ref();
    let chat = chat && info.is_some_and(|i| i.chat.is_some());
    let live_chat = chat && info.is_some_and(|i| i.chat.as_ref().is_some_and(|c| !c.1));
    if chat && !two {
        b.prims.push(Prim::Chat { x: 1, y: y - 1, w: lw, h: 12, input: live_chat });
        if live_chat {
            b.tgt(1, y - 1 + 11, lw, 1, Act::ChatSend);
        }
        y += 12;
    }
    let title = info.map(|i| i.title.clone()).unwrap_or_else(|| wt.video.title.clone());
    y += b.para(1, y, lw, &title, bold, 3);
    y += 1;
    if let Some(i) = info {
        if let Some(a) = &i.avatar {
            b.prims.push(Prim::Image { x: 1, y, w: 4, h: 2, url: a.clone(), badge: None, progress: None });
        }
        let name = trunc(&i.channel, (lw as usize).saturating_sub(40).max(10));
        let nw = name.width() as u16;
        b.line(6, y, vec![Span::styled(name, bold)]);
        b.tgt(6, y, nw, 1, Act::Channel(i.channel_id.clone()));
        b.line(6, y + 1, vec![Span::styled(i.subs.clone(), st(META))]);
        let subbed = b.store.is_sub(&i.channel_id);
        let mut bx = 6 + nw.max(i.subs.width() as u16) + 3;
        bx += b.button(bx, y, if subbed { "Subscribed ✓" } else { "Subscribe" }, Act::ToggleSub(i.channel_id.clone(), i.channel.clone()), !subbed) + 1;
        let saved = b.store.later.iter().any(|v| v.id == wt.video.id);
        let mut v = wt.video.clone();
        if v.title.is_empty() {
            v.title = i.title.clone();
            v.channel = i.channel.clone();
        }
        bx += b.button(bx, y, if saved { "✓ Saved" } else { "+ Watch later" }, Act::ToggleLater(v.clone()), false) + 1;
        bx += b.button(bx, y, "≡+ Save", Act::SaveTo(v), false) + 1;
        let rating = wt.rating.as_deref().unwrap_or("none");
        let likes = if i.likes.is_empty() { "Like".to_string() } else { i.likes.clone() };
        bx += b.button(bx, y, &format!("▲ {likes}"), Act::Rate("like".into()), rating == "like");
        b.button(bx, y, "▼", Act::Rate("dislike".into()), rating == "dislike");
        y += 3;
        // Description panel
        let mut stats = Vec::new();
        if !i.views.is_empty() {
            stats.push(i.views.clone());
        }
        if !i.date.is_empty() {
            stats.push(i.date.clone());
        }
        if !i.likes.is_empty() {
            stats.push(format!("{} likes", i.likes));
        }
        let dw = lw.saturating_sub(2);
        let mut dy = y + 1;
        let start = b.prims.len();
        b.line(2, dy, vec![Span::styled(stats.join("  ·  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]);
        dy += 1;
        let lines = wrap(&i.description, dw as usize - 1, if wt.desc_open { 4000 } else { 3 });
        for l in &lines {
            if wt.desc_open {
                if let Some(t) = timestamp(l) {
                    let tw = l.split_whitespace().next().map(|s| s.width()).unwrap_or(4) as u16;
                    b.line(2, dy, vec![Span::styled(l.clone(), st(Color::Rgb(120, 170, 255)))]);
                    b.tgt(2, dy, tw.max(4), 1, Act::Seek(t));
                    dy += 1;
                    continue;
                }
            }
            b.line(2, dy, vec![Span::styled(l.clone(), st(Color::Rgb(220, 220, 220)))]);
            dy += 1;
        }
        let more = if wt.desc_open { "Show less" } else { "…more" };
        b.line(2, dy, vec![Span::styled(more.to_string(), bold)]);
        b.tgt(2, dy, more.width() as u16, 1, Act::ExpandDesc);
        dy += 2;
        b.prims.insert(start, Prim::Fill { x: 1, y, w: lw, h: (dy - y) as u16, color: PANEL });
        y = dy + 1;
    } else if page.error.is_none() {
        b.line(1, y, vec![Span::styled("Loading…".to_string(), st(META))]);
        y += 2;
    }
    if let Some(e) = &page.error {
        b.line(1, y, vec![Span::styled(format!("⚠ {e}"), st(ACCENT))]);
        y += 2;
    }

    let related: Vec<Entry> = info.map(|i| i.related.entries.clone()).unwrap_or_default();
    let rel_videos: Vec<VideoItem> = Vec::new();
    let side = |b: &mut B, x: u16, mut y: usize, cw: u16, limit: usize| -> usize {
        if !wt.queue.is_empty() {
            b.line(x, y, vec![
                Span::styled("Playlist", bold),
                Span::styled(format!("  {}/{}", wt.qpos + 1, wt.queue.len()), st(META)),
            ]);
            y += 2;
            let tw = (cw * 40 / 100).clamp(12, 24);
            let lo = wt.qpos.saturating_sub(2);
            for (i, v) in wt.queue.iter().enumerate().skip(lo).take(8) {
                let mark = if i == wt.qpos { "▶ " } else { "" };
                let item = Item::Video(v.clone());
                let h = b.list_card(x, y, cw, tw, &item, &wt.queue, Some(format!("{mark}{}. ", i + 1)));
                y += h + 1;
            }
            y += 1;
        }
        b.line(x, y, vec![
            Span::styled("Up next", bold),
            Span::styled(format!("   autoplay {} (a)", if autoplay { "on" } else { "off" }), st(DIM)),
        ]);
        y += 2;
        let tw = (cw * 42 / 100).clamp(12, 26);
        for e in related.iter().take(limit) {
            if let Entry::Item(it) = e {
                y += b.list_card(x, y, cw, tw, it, &rel_videos, None) + 1;
            }
        }
        y
    };

    let (mut left_end, mut right_end) = (y, 0usize);
    if two {
        let rx = lw + 3;
        let top = if chat {
            let ch = ((vh as usize) * 55 / 100).clamp(10, 30);
            b.prims.push(Prim::Chat { x: rx, y: 0, w: w - rx, h: ch as u16, input: live_chat });
            if live_chat {
                b.tgt(rx, ch - 1, w - rx - 1, 1, Act::ChatSend);
            }
            ch + 1
        } else {
            0
        };
        right_end = side(b, rx, top, w - rx, 40);
    } else if info.is_some() {
        left_end = side(b, 1, y, lw, 6) + 1;
    }
    let mut y = left_end;
    if info.is_some() {
        let count = wt.comment_count.clone().unwrap_or_default();
        b.line(1, y, vec![Span::styled(if count.is_empty() { "Comments".to_string() } else { count }, bold)]);
        y += 2;
        if info.is_some_and(|i| i.comments_token.is_some()) {
            let prompt = match &ctx.account {
                Some(_) => "Add a comment…",
                None => "Add a comment…  (sign in to comment)",
            };
            let pw = lw.saturating_sub(1);
            b.prims.push(Prim::Fill { x: 1, y, w: pw, h: 1, color: PANEL });
            b.line(2, y, vec![Span::styled(prompt.to_string(), st(DIM))]);
            b.tgt(1, y, pw, 1, Act::Compose);
            y += 2;
        }
        if wt.comments.is_empty() {
            let msg = if wt.comments_loading { "Loading comments…" } else if info.is_some_and(|i| i.comments_token.is_none()) { "Comments are turned off." } else { "No comments yet." };
            b.line(1, y, vec![Span::styled(msg.to_string(), st(META))]);
            y += 2;
        }
        for (idx, c) in wt.comments.iter().enumerate() {
            let ind = 1 + c.level as u16 * 4;
            let cw = lw.saturating_sub(ind - 1);
            let name_style = if c.creator { Style::default().fg(Color::Black).bg(Color::Rgb(200, 200, 200)) } else { bold };
            b.line(ind, y, vec![
                Span::styled(c.author.clone(), name_style),
                Span::styled(format!("  {}", c.time), st(DIM)),
            ]);
            y += 1;
            y += b.para(ind, y, cw, &c.text, st(Color::Rgb(225, 225, 225)), 12);
            let mut tail = Vec::new();
            if !c.likes.is_empty() && c.likes != "0" {
                tail.push(Span::styled(format!("♥ {}   ", c.likes), st(DIM)));
            }
            let replies = (c.replies > 0 && c.replies_token.is_some()).then(|| format!("▾ {} repl{}", c.replies, if c.replies == 1 { "y" } else { "ies" }));
            let reply_x = ind + tail.iter().map(|s| s.content.width() as u16).sum::<u16>();
            tail.push(Span::styled("Reply".to_string(), st(META)));
            tail.push(Span::raw("   "));
            let tx = reply_x + 8;
            if let Some(r) = &replies {
                tail.push(Span::styled(r.clone(), st(Color::Rgb(120, 170, 255))));
            }
            b.line(ind, y, tail);
            if !c.id.is_empty() {
                b.tgt(reply_x, y, 5, 1, Act::ReplyTo(idx));
            }
            if let Some(r) = replies {
                b.tgt(tx, y, r.width() as u16, 1, Act::Replies(idx));
            }
            y += 1;
            y += 1;
        }
        if page.more_loading {
            b.line(1, y, vec![Span::styled("Loading more…".to_string(), st(DIM))]);
            y += 2;
        }
    }
    y.max(right_end)
}

pub const HELP_TEXT: &str = "\
Keys — browsing
  / or Ctrl-L      search or open a URL (↑↓ pick a suggestion, Enter go, Esc cancel)
  arrows / hjkl    move focus around the page        Enter / click   open
  PgUp PgDn g G    scroll                           mouse wheel      scroll
  H / Backspace    back                             L                forward
  r                reload                           Ctrl-B           toggle sidebar
  Tab              jump between sidebar and page     ?                this help
  Ctrl-T           new tab        Ctrl-W  close tab        J / K     next / previous tab
  t                open focused item in a new tab
  w                add focused video to Watch later
  c                go to the focused video's channel
  m / right-click  song menu (YouTube Music): play next, add to queue, go to album…
  s                subscribe / unsubscribe (watch page, channel page, focused channel)
  q or Ctrl-C      quit

Keys — player (anywhere while something is playing)
  space            pause / play                     f                fullscreen (Esc leaves)
  , .              back / forward 10 s              < >              back / forward 60 s
  0-9              jump to 0%..90%                  n                next video
  x                stop and close the player        a                autoplay on / off
  Like/dislike buttons and 'Add a comment…' / Reply on the watch page need
  sign-in: run `ytui --login` (setup steps on the Account page / --help-login)
  composer: Enter posts, Alt-Enter new line, Esc cancels, paste works
  + -              volume up / down (0-150%)        M                mute
  C                show / hide live chat            I                send a live chat message (signed in)
  P                save the focused / current video to a playlist (signed in)
  E                auto-exposure on / off (lifts dark videos; 'AE' on the player bar)
  S                captions: your language first, then each track, then off (remembered)
  click the progress bar to seek; click the miniplayer to return to the video
  move the mouse over the video for its controls: previous, play / pause, next, mute, CC, full

Video is drawn as 24-bit colour half blocks, two pixels a cell. Only cells
that change are sent, sized to what the terminal can take, so frames don't
pile up; fast motion gets a little coarser instead.

Pages
  Home         For you (from your history and subscriptions) and topic chips
  Subscriptions  latest uploads from channels you subscribed to (kept locally)
  Music        YouTube Music: Home, Explore, charts, search, artists, albums,
               your library, and a player with Up next, Lyrics and Related
  History      everything you watched, with resume positions
  Watch later  your saved videos
";

// ------------------------------------------------------------ YouTube Music

const MUSIC_RED: Color = Color::Rgb(255, 0, 51);

/// The YouTube Music bar over every music page: its logo, Home, Explore,
/// Library and a search button.
fn music_bar(b: &mut B, y: usize, route: &MRoute) -> usize {
    let mut x = 1;
    b.line(x, y, vec![Span::styled("♫ ".to_string(), Style::default().fg(MUSIC_RED).add_modifier(Modifier::BOLD)), Span::styled("Music".to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]);
    b.tgt(x, y, 7, 1, Act::MusicGo(MRoute::Home));
    x += 10;
    for (label, r) in [("Home", MRoute::Home), ("Explore", MRoute::Explore), ("Library", MRoute::Library(0))] {
        let on = match (&r, route) {
            (MRoute::Library(_), MRoute::Library(_)) => true,
            _ => &r == route,
        };
        x += b.button(x, y, label, Act::MusicGo(r), on) + 1;
    }
    b.button(x + 2, y, "⌕ Search music", Act::MusicSearch, false);
    2
}

/// Square album art: `w` cells wide is `w / 2` rows high.
fn art(b: &mut B, x: u16, y: usize, w: u16, url: Option<&String>) -> u16 {
    let h = (w / 2).max(1);
    match url {
        Some(u) => b.prims.push(Prim::Image { x, y, w, h, url: u.clone(), badge: None, progress: None }),
        None => b.prims.push(Prim::Fill { x, y, w, h, color: PANEL }),
    }
    h
}

/// Artist banner, or album / playlist art with its details and buttons.
fn music_header(b: &mut B, y0: usize, w: u16, h: &Header) -> usize {
    let bold = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let mut y = y0;
    let buttons = |b: &mut B, x: u16, y: usize| {
        let mut bx = x;
        if let Some(p) = &h.play {
            bx += b.button(bx, y, if h.banner { "⇄ Shuffle" } else { "▶ Play" }, Act::MusicPlaylist(p.clone()), true) + 1;
        }
        if let Some(p) = &h.shuffle {
            bx += b.button(bx, y, if h.banner { "◉ Radio" } else { "⇄ Shuffle" }, Act::MusicPlaylist(p.clone()), false) + 1;
        }
        if let Some(p) = &h.radio {
            bx += b.button(bx, y, "◉ Radio", Act::MusicPlaylist(p.clone()), false) + 1;
        }
        if let Some(c) = &h.channel {
            let subbed = b.store.is_sub(c);
            let label = match (&h.subscribers, subbed) {
                (_, true) => "Subscribed ✓".to_string(),
                (Some(n), false) => format!("Subscribe {n}"),
                (None, false) => "Subscribe".to_string(),
            };
            b.button(bx, y, &label, Act::ToggleSub(c.clone(), h.title.clone()), !subbed);
        }
    };
    // Title-only headers (charts, moods, history): a heading.
    if h.thumb.is_none() && h.play.is_none() {
        b.line(1, y, vec![Span::styled(h.title.clone(), bold)]);
        if !h.subtitle.is_empty() {
            b.line(1, y + 1, vec![Span::styled(trunc(&h.subtitle, w as usize), st(META))]);
            return 3;
        }
        return 2;
    }
    if h.banner {
        if let Some(u) = &h.thumb {
            let bh = ((w as f32) * 0.16).clamp(6.0, 16.0) as u16;
            b.prims.push(Prim::Image { x: 1, y, w, h: bh, url: u.clone(), badge: None, progress: None });
            y += bh as usize + 1;
        }
        b.line(1, y, vec![Span::styled(h.title.clone(), bold)]);
        let mut meta = Vec::new();
        if let Some(s) = &h.subscribers {
            meta.push(format!("{s} subscribers"));
        }
        if !h.subtitle.is_empty() {
            meta.push(h.subtitle.clone());
        }
        y += 1;
        if !meta.is_empty() {
            b.line(1, y, vec![Span::styled(trunc(&meta.join(" · "), w as usize), st(META))]);
            y += 1;
        }
        if !h.description.is_empty() {
            y += b.para(1, y, w, &h.description, st(DIM), 3);
        }
        y += 1;
        buttons(b, 1, y);
        return y + 2 - y0;
    }
    let aw = (w / 4).clamp(14, 32);
    let ah = art(b, 1, y, aw, h.thumb.as_ref());
    let tx = 1 + aw + 3;
    let tw = w.saturating_sub(aw + 3);
    let mut ty = y;
    ty += b.para(tx, ty, tw, &h.title, bold, 2);
    if let Some((name, id)) = &h.artist {
        let n = trunc(name, tw as usize);
        let nw = n.width() as u16;
        b.line(tx, ty, vec![Span::styled(n, Style::default().fg(Color::White))]);
        b.tgt(tx, ty, nw, 1, Act::MusicGo(MRoute::Browse(Browse::new(id))));
        ty += 1;
    }
    for t in [&h.subtitle, &h.second] {
        if !t.is_empty() {
            b.line(tx, ty, vec![Span::styled(trunc(t, tw as usize), st(META))]);
            ty += 1;
        }
    }
    if !h.description.is_empty() {
        ty += 1;
        ty += b.para(tx, ty, tw, &h.description, st(DIM), 3);
    }
    ty += 1;
    buttons(b, tx, ty);
    ty += 2;
    y + (ah as usize).max(ty - y) + 1 - y0
}

/// One song row: art (or its number), title, artists · album · plays, and
/// the duration at the right.
fn song_row(b: &mut B, x: u16, y: usize, w: u16, s: &Song, numbered: bool, now: bool, act: Act) {
    let mut tx = x;
    if numbered {
        // Album tracks: the number, as YouTube Music lists them.
        b.line(x, y, vec![Span::styled(format!("{:>3}", s.index.as_deref().unwrap_or("")), st(META))]);
        tx += 5;
    } else {
        art(b, x, y, 4, s.thumb.as_ref());
        tx += 6;
        if let Some(i) = &s.index {
            // Chart positions.
            b.line(tx, y, vec![Span::styled(format!("{i:>3} "), st(META))]);
            tx += 4;
        }
    }
    let dur = s.video.duration.clone().unwrap_or_default();
    // The ⋮ menu at the row's end, then the duration.
    b.line(x + w - 1, y, vec![Span::styled("⋮".to_string(), st(META))]);
    b.tgt(x + w - 2, y, 2, 2, Act::SongMenu(Box::new(s.clone())));
    let w = w - 3;
    let tw = (x + w).saturating_sub(tx + dur.width() as u16 + 2) as usize;
    let title_style = if now { Style::default().fg(MUSIC_RED).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) };
    let mark = if now { "▶ " } else { "" };
    b.line(tx, y, vec![Span::styled(trunc(&format!("{mark}{}", s.video.title), tw), title_style)]);
    let mut meta = Vec::new();
    if !s.video.channel.is_empty() {
        meta.push(s.video.channel.clone());
    }
    if let Some((a, _)) = &s.album {
        if *a != s.video.title && !numbered {
            meta.push(a.clone());
        }
    }
    if let Some(v) = &s.video.views {
        meta.push(v.clone());
    }
    b.line(tx, y + 1, vec![Span::styled(trunc(&meta.join(" · "), tw), st(META))]);
    if !dur.is_empty() {
        b.line(x + w - dur.width() as u16, y, vec![Span::styled(dur, st(META))]);
    }
    b.tgt(x, y, w + 1, 2, act);
}

/// A card: square art, title, subtitle.
fn music_card(b: &mut B, x: u16, y: usize, w: u16, c: &music::Card, act: Act) -> usize {
    let ah = art(b, x, y, w, c.thumb.as_ref());
    let mut yy = y + ah as usize + 1;
    let style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    yy += b.para(x, yy, w, &c.title, style, 2);
    let mut sub = c.subtitle.clone();
    if c.artist && !sub.starts_with("Artist") {
        sub = if sub.is_empty() { "Artist".into() } else { format!("Artist • {sub}") };
    }
    yy += b.para(x, yy, w, &sub, st(META), 2);
    if matches!(c.kind, CardKind::Play { .. }) {
        b.line(x, y, vec![Span::styled(" ▶ ".to_string(), Style::default().fg(Color::White).bg(Color::Rgb(0, 0, 0)))]);
    }
    b.tgt(x, y, w, (yy - y) as u16, act);
    yy - y
}

/// A page's sections. `all` shows every item (a "more" page); otherwise a
/// row of cards and a few songs each, as YouTube Music's shelves do.
fn music_sections(b: &mut B, y0: usize, x0: u16, w: u16, page: &MusicPage, all: bool, act: &dyn Fn(usize, usize) -> Act, now: Option<&str>) -> usize {
    let bold = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let mut y = y0;
    // Album and playlist tracks: one numbered list, in full.
    let tracks = page.header.as_ref().is_some_and(|h| !h.banner && h.thumb.is_some());
    for (si, sec) in page.sections.iter().enumerate() {
        if !sec.title.is_empty() {
            b.line(x0, y, vec![Span::styled(trunc(&sec.title, w as usize - 8), bold)]);
            if let Some(m) = &sec.more {
                let label = "More ›";
                let mx = x0 + w - label.width() as u16;
                b.line(mx, y, vec![Span::styled(label.to_string(), st(META))]);
                let target = match m.id.strip_prefix("filter:").and_then(|i| i.parse().ok()) {
                    Some(i) => Act::MusicFilter(i),
                    None => Act::MusicGo(MRoute::Browse(m.clone())),
                };
                b.tgt(mx, y, label.width() as u16, 1, target);
            }
            y += 2;
        }
        if let Some(t) = &sec.text {
            y += b.para(x0, y, w, t, st(Color::Rgb(220, 220, 220)), if all { 400 } else { 6 }) + 1;
            continue;
        }
        let songs = sec.items.iter().filter(|i| matches!(i, MItem::Song(_))).count();
        let navs = sec.items.iter().filter(|i| matches!(i, MItem::Nav { .. })).count();
        if navs > 0 && navs == sec.items.len() {
            // Mood and genre buttons, each with its colour stripe.
            let (mut x, mut row) = (x0, 0);
            let cap = if all { usize::MAX } else { 4 };
            for (ii, it) in sec.items.iter().enumerate() {
                let MItem::Nav { label, color, .. } = it else { continue };
                let bw = (label.width() as u16 + 4).max(14);
                if x > x0 && x + bw > x0 + w {
                    x = x0;
                    row += 2;
                    if row / 2 >= cap {
                        break;
                    }
                }
                let c = color.map(|c| Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(META);
                let pad = " ".repeat((bw as usize).saturating_sub(label.width() + 3));
                b.line(x, y + row, vec![Span::styled("▍".to_string(), Style::default().fg(c).bg(BUTTON)), Span::styled(format!(" {label}{pad} "), Style::default().fg(Color::White).bg(BUTTON))]);
                b.tgt(x, y + row, bw, 1, act(si, ii));
                x += bw + 1;
            }
            y += row + 2;
        } else if songs * 2 >= sec.items.len() {
            // Songs: in columns where there's room, like Quick picks.
            let full = all || tracks;
            let cols = if full || w < 90 { 1 } else { ((w + 2) / 46).clamp(1, 3) as usize };
            let per_col = if full { usize::MAX } else { 4 };
            let shown: Vec<(usize, &MItem)> = sec.items.iter().enumerate().take(cols.saturating_mul(per_col)).collect();
            let rows = shown.len().div_ceil(cols);
            let cw = (w + 2) / cols as u16 - 2;
            for (k, (ii, it)) in shown.iter().enumerate() {
                let (c, r) = (k / rows, k % rows);
                let x = x0 + c as u16 * (cw + 2);
                match it {
                    MItem::Song(s) => {
                        let album_track = tracks && s.thumb.is_some() && s.thumb == page.header.as_ref().and_then(|h| h.thumb.clone());
                        song_row(b, x, y + r * 3, cw, s, album_track, now == Some(s.video.id.as_str()), act(si, *ii))
                    }
                    MItem::Card(c) => {
                        // A card among songs (search): a row with its art.
                        art(b, x, y + r * 3, 4, c.thumb.as_ref());
                        b.line(x + 6, y + r * 3, vec![Span::styled(trunc(&c.title, cw as usize - 6), bold)]);
                        b.line(x + 6, y + r * 3 + 1, vec![Span::styled(trunc(&c.subtitle, cw as usize - 6), st(META))]);
                        b.tgt(x, y + r * 3, cw, 2, act(si, *ii));
                    }
                    MItem::Nav { .. } => {}
                }
            }
            y += rows * 3 + 1;
        } else {
            // Cards: a grid, one or two rows unless showing everything.
            let n = ((w + 2) / 24).max(2) as usize;
            let cw = (w + 2) / n as u16 - 2;
            let cap = if all || page.sections.len() <= 2 { usize::MAX } else if sec.items.len() > n * 2 { n * 2 } else { usize::MAX };
            let mut row_h = 0;
            let mut col = 0;
            for (ii, it) in sec.items.iter().enumerate().take(cap) {
                let x = x0 + col as u16 * (cw + 2);
                let h = match it {
                    MItem::Card(c) => music_card(b, x, y, cw, c, act(si, ii)),
                    MItem::Song(s) => {
                        let c = music::Card { title: s.video.title.clone(), subtitle: s.video.channel.clone(), thumb: s.thumb.clone(), kind: CardKind::Play { video: s.video.clone(), playlist: s.playlist.clone() }, artist: false };
                        music_card(b, x, y, cw, &c, act(si, ii))
                    }
                    MItem::Nav { .. } => 0,
                };
                row_h = row_h.max(h);
                col += 1;
                if col == n {
                    y += row_h + 1;
                    col = 0;
                    row_h = 0;
                }
            }
            if col > 0 {
                y += row_h + 1;
            }
            y += 1;
        }
    }
    y
}

fn music_layout(b: &mut B, page: &Page, route: &MRoute, mp: Option<&MusicPage>, w: u16, ctx: &Ctx) -> usize {
    let mut y = music_bar(b, 0, route) + 1;
    match route {
        MRoute::Search { filter, .. } => {
            let labels: Vec<String> = music::FILTERS.iter().map(|f| f.0.to_string()).collect();
            y += b.chips(1, y, w, &labels, Some(*filter), Act::MusicFilter) + 1;
        }
        MRoute::Library(i) => {
            if ctx.yt_account.is_none() && ctx.yt_cookies.is_none() {
                b.line(1, y, vec![Span::styled("Sign in to see your library: playlists, liked songs, albums, artists and history.".to_string(), st(META))]);
                b.button(1, y + 2, "Sign in", Act::SignIn, true);
                return y + 4;
            }
            let labels: Vec<String> = music::LIBRARY.iter().map(|l| l.0.to_string()).collect();
            y += b.chips(1, y, w, &labels, Some(*i), Act::MusicLib) + 1;
        }
        _ => {}
    }
    let Some(mp) = mp else {
        return b.status(y, page, "", false);
    };
    if let Some(h) = &mp.header {
        y += music_header(b, y, w.saturating_sub(1), h) + 1;
    }
    y = b.status(y, page, "Nothing here.", !mp.sections.is_empty());
    // A page reached through "More" is one long section: show it all.
    let all = matches!(route, MRoute::Library(_)) || matches!(route, MRoute::Search { filter, .. } if *filter > 0) || mp.sections.len() == 1;
    // One column clear of the scrollbar.
    music_sections(b, y, 1, w.saturating_sub(1), mp, all, &|s, i| Act::MusicItem(s, i), None)
}

/// The music player: the picture, the song, and Up next / Lyrics / Related.
fn music_watch_layout(b: &mut B, wt: &Watch, m: &MusicNow, w: u16, vh: u16) -> usize {
    let bold = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let two = w >= 110;
    let lw = if two { w * 56 / 100 } else { w } - 1;
    let max_h = ((vh as f32) * 0.62) as u16;
    let mut ph = ((lw as f32) * 0.28125).round() as u16;
    let mut pw = lw;
    if ph > max_h {
        ph = max_h.max(4);
        pw = ((ph as f32) / 0.28125).round() as u16;
    }
    let px = 1 + (lw - pw.min(lw)) / 2;
    b.prims.push(Prim::Player { x: px, y: 0, w: pw.min(lw), h: ph, id: wt.video.id.clone() });
    b.tgt(px, 0, pw.min(lw), ph, Act::Player);
    b.prims.push(Prim::PlayerBar { x: 1, y: ph as usize, w: lw, id: wt.video.id.clone() });
    b.tgt(1, ph as usize, lw, 1, Act::PlayerBar);
    let mut y = ph as usize + 2;
    let title = wt.info.as_ref().map(|i| i.title.clone()).filter(|_| wt.video.title.is_empty()).unwrap_or_else(|| wt.video.title.clone());
    y += b.para(1, y, lw, &title, bold, 2);
    // Artist and album, each a link.
    let mut x = 1u16;
    let artist = if wt.video.channel.is_empty() { wt.info.as_ref().map(|i| i.channel.clone()).unwrap_or_default() } else { wt.video.channel.clone() };
    let aw = artist.width() as u16;
    b.line(x, y, vec![Span::styled(artist.clone(), st(Color::White))]);
    if let Some(id) = wt.video.channel_id.clone().or_else(|| wt.info.as_ref().map(|i| i.channel_id.clone())).filter(|i| !i.is_empty()) {
        b.tgt(x, y, aw.max(1), 1, Act::MusicGo(MRoute::Browse(Browse::new(&id))));
    }
    x += aw;
    if let Some((name, id)) = &m.album {
        b.line(x, y, vec![Span::styled(" · ".to_string(), st(META)), Span::styled(name.clone(), st(META))]);
        b.tgt(x + 3, y, name.width() as u16, 1, Act::MusicGo(MRoute::Browse(Browse::new(id))));
    }
    y += 2;
    let rating = wt.rating.as_deref().unwrap_or("none");
    let mut bx = 1;
    bx += b.button(bx, y, "▲ Like", Act::Rate("like".into()), rating == "like") + 1;
    bx += b.button(bx, y, "▼", Act::Rate("dislike".into()), rating == "dislike") + 1;
    bx += b.button(bx, y, "≡+ Save", Act::SaveTo(wt.video.clone()), false) + 1;
    let saved = b.store.later.iter().any(|v| v.id == wt.video.id);
    b.button(bx, y, if saved { "✓ Saved" } else { "+ Later" }, Act::ToggleLater(wt.video.clone()), false);
    y += 2;
    // Tabs: beside the player when wide, else below.
    let (tx, tw, mut ty) = if two { (lw + 3, w.saturating_sub(lw + 4), 0) } else { (1, lw, y + 1) };
    let mut x = tx;
    for (i, t) in ["UP NEXT", "LYRICS", "RELATED"].iter().enumerate() {
        let on = i == m.tab;
        let style = if on { Style::default().fg(Color::White).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { st(META) };
        b.line(x, ty, vec![Span::styled(t.to_string(), style)]);
        b.tgt(x, ty, t.width() as u16, 1, Act::MusicTab(i));
        x += t.width() as u16 + 4;
    }
    b.line(tx, ty + 1, vec![Span::styled("─".repeat(tw as usize), st(Color::Rgb(60, 60, 70)))]);
    ty += 3;
    match m.tab {
        0 => match &m.next {
            Some(n) if !n.queue.is_empty() => {
                for (i, s) in n.queue.iter().enumerate() {
                    // No track or chart numbers in the queue.
                    let s = Song { index: None, ..s.clone() };
                    song_row(b, tx, ty, tw, &s, false, s.video.id == wt.video.id, Act::MusicQueue(i));
                    ty += 3;
                }
            }
            Some(_) => {
                b.line(tx, ty, vec![Span::styled("Nothing up next.".to_string(), st(META))]);
                ty += 2;
            }
            None => {
                b.line(tx, ty, vec![Span::styled("Loading…".to_string(), st(META))]);
                ty += 2;
            }
        },
        1 => match &m.lyrics {
            Some(Ok(l)) => {
                for line in l.text.lines() {
                    ty += b.para(tx, ty, tw, line, st(Color::Rgb(230, 230, 230)), 20).max(1);
                }
                ty += 1;
                b.line(tx, ty, vec![Span::styled(l.source.clone(), st(DIM))]);
                ty += 2;
            }
            Some(Err(e)) => {
                b.line(tx, ty, vec![Span::styled(e.clone(), st(META))]);
                ty += 2;
            }
            None => {
                b.line(tx, ty, vec![Span::styled("Loading lyrics…".to_string(), st(META))]);
                ty += 2;
            }
        },
        _ => match &m.related {
            Some(Ok(p)) => ty = music_sections(b, ty, tx, tw, p, false, &|s, i| Act::MusicRelated(s, i), None),
            Some(Err(e)) => {
                b.line(tx, ty, vec![Span::styled(e.clone(), st(META))]);
                ty += 2;
            }
            None => {
                b.line(tx, ty, vec![Span::styled("Loading…".to_string(), st(META))]);
                ty += 2;
            }
        },
    }
    y.max(ty)
}
