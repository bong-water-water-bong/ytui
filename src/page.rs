//! Page model and layout. Every page is laid out into a `Doc`: positioned
//! primitives (text, images, the player) plus clickable / focusable targets,
//! in document coordinates. The app scrolls the doc, draws what is visible,
//! hit-tests the mouse against targets and moves keyboard focus spatially
//! between them, like a browser.

use crate::api::{ChannelPage, Comment, Entry, Feed, Item, PlaylistPage, VideoItem, WatchInfo};
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
            Kind::Watch(w) => w.info.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| w.video.title.clone()),
            Kind::Subs { .. } => "Subscriptions".into(),
            Kind::History => "History".into(),
            Kind::Later => "Watch later".into(),
            Kind::Help => "Help".into(),
        }
    }

    pub fn address(&self) -> String {
        match &self.kind {
            Kind::Home { .. } => "ytui://home".into(),
            Kind::Search { q, .. } => format!("youtube.com/results?search_query={}", crate::api::urlencode(q)),
            Kind::Channel { id, .. } => format!("youtube.com/channel/{id}"),
            Kind::Playlist { id, .. } => format!("youtube.com/playlist?list={id}"),
            Kind::Watch(w) => format!("youtube.com/watch?v={}", w.video.id),
            Kind::Subs { .. } => "ytui://subscriptions".into(),
            Kind::History => "ytui://history".into(),
            Kind::Later => "ytui://watch-later".into(),
            Kind::Help => "ytui://help".into(),
        }
    }

    /// Continuation token for infinite scroll, if any.
    pub fn more_token(&self) -> Option<&str> {
        match &self.kind {
            Kind::Home { feed, .. } | Kind::Search { feed, .. } => feed.cont.as_deref(),
            Kind::Channel { page: Some(p), .. } => p.feed.cont.as_deref(),
            Kind::Playlist { page: Some(p), .. } => p.feed.cont.as_deref(),
            Kind::Watch(w) => w.comments_cont.as_deref(),
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
    ExpandDesc,
    Replies(usize),
    Seek(f64),
    Player,
    PlayerBar,
    PlayAll,
    ClearHistory,
}

#[derive(Clone)]
pub enum Prim {
    Text { x: u16, y: usize, line: Line<'static> },
    Image { x: u16, y: usize, w: u16, h: u16, url: String, badge: Option<(String, Color)>, progress: Option<f32> },
    Fill { x: u16, y: usize, w: u16, h: u16, color: Color },
    Player { x: u16, y: usize, w: u16, h: u16, id: String },
    PlayerBar { x: u16, y: usize, w: u16, id: String },
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
        } else if page.more_loading {
            Some(("Loading more…".to_string(), DIM))
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
pub fn layout(page: &Page, store: &Store, w: u16, vh: u16, autoplay: bool) -> Doc {
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
        Kind::Watch(wt) => y = watch_layout(&mut b, page, wt, w, vh, autoplay),
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
                let labels: Vec<String> = store.subs.iter().map(|c| c.name.clone()).collect();
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

fn watch_layout(b: &mut B, page: &Page, wt: &Watch, w: u16, vh: u16, autoplay: bool) -> usize {
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
        b.button(bx, y, if saved { "✓ Saved" } else { "+ Watch later" }, Act::ToggleLater(v), false);
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
        right_end = side(b, rx, 0, w - rx, 40);
    } else if info.is_some() {
        left_end = side(b, 1, y, lw, 6) + 1;
    }
    let mut y = left_end;
    if info.is_some() {
        let count = wt.comment_count.clone().unwrap_or_default();
        b.line(1, y, vec![Span::styled(if count.is_empty() { "Comments".to_string() } else { count }, bold)]);
        y += 2;
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
            let tx = ind + tail.iter().map(|s| s.content.width() as u16).sum::<u16>();
            if let Some(r) = &replies {
                tail.push(Span::styled(r.clone(), st(Color::Rgb(120, 170, 255))));
            }
            if !tail.is_empty() {
                b.line(ind, y, tail);
                if let Some(r) = replies {
                    b.tgt(tx, y, r.width() as u16, 1, Act::Replies(idx));
                }
                y += 1;
            }
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
  s                subscribe / unsubscribe (watch page, channel page, focused channel)
  q or Ctrl-C      quit

Keys — player (anywhere while something is playing)
  space            pause / play                     f                fullscreen (Esc leaves)
  , .              back / forward 10 s              < >              back / forward 60 s
  0-9              jump to 0%..90%                  n                next video
  x                stop and close the player        a                autoplay on / off
  m                cycle render mode                [ ] { }          ascii contrast / edges
  click the progress bar to seek; click the miniplayer to return to the video

Render modes
  ascii      two-tone shape-matched ASCII (glyphs follow edges)
  classic    shape-matched ASCII, coloured ink on dark ground
  sextant    2x3 two-colour mosaic per cell (sharpest; needs a font with sextants)
  quadrant   2x2 mosaic        halfblock   top/bottom halves        braille   2x4 dots

Pages
  Home         For you (from your history and subscriptions) and topic chips
  Subscriptions  latest uploads from channels you subscribed to (kept locally)
  History      everything you watched, with resume positions
  Watch later  your saved videos
";
