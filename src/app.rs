//! The browser: tabs with back/forward history, an address bar with search
//! suggestions, a sidebar, pages laid out as scrollable documents, mouse and
//! spatial keyboard navigation, and one video player that plays inline on
//! its watch page, in a miniplayer elsewhere, or fullscreen.

use crate::api::{self, Api, ChannelPage, Comment, CommentBatch, Entry, Feed, Item, PlaylistPage, Role, Target, VideoItem, WatchInfo};
use crate::auth::{Auth, MyPlaylist, PlItem};
use crate::chat::{self, Chat};
use crate::engine3d::Engine;
use crate::glyphs::{CH as GH, CW as GW};
use crate::scenes::{Analyzer, Kind as SceneKind, Orbit, Scenes};
use crate::page::{self, chip_query, layout, Act, Ctx, Doc, Kind, Page, Prim, Watch, ACCENT, CHIPS, DIM, FILTERS, META};
use crate::player::{fit, Opts, Player};
use crate::render::{Cell, Mode, Renderer};
use crate::store::Store;
use crate::thumbs::Thumbs;
use crate::yt::{self, fmt_time};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

type R<T> = std::result::Result<T, String>;

pub enum Msg {
    Wake,
    Load(u64, Load),
    Stream(String, R<yt::Stream>),
    Suggest(String, Vec<String>),
    Related(String, Vec<VideoItem>, Option<(String, bool)>),
    Open(R<Target>, bool),
    Chat(String, chat::Update),
    /// Your current rating of the watch page's video.
    Rating(u64, String),
    /// Result of rating: (page, rating before the change, outcome).
    Rated(u64, String, R<()>),
    /// A comment or reply was posted; Some(idx) = reply under comment idx.
    Posted(u64, Option<usize>, R<Comment>),
    /// Account subscriptions: (channel id, title, subscription id); bool = user asked.
    SubsSynced(R<Vec<(String, String, String)>>, bool),
    /// Remote subscribe/unsubscribe finished: (channel, name, wanted on, new sub id).
    SubDone(String, String, bool, R<String>),
    ChatSent(R<()>),
    /// Playlists for the Save dialog of this video.
    DlgPlaylists(String, R<Vec<MyPlaylist>>),
    /// Is the dialog's video in playlist? (video, playlist, item id)
    DlgMember(String, String, Option<String>),
    /// Add/remove from the dialog finished: (video, playlist, title, new item id or None if removed).
    DlgToggled(String, String, String, R<Option<String>>),
    /// A playlist edit finished; flash this and refresh playlist pages.
    PlDone(R<String>),
}

#[derive(Clone, Copy, PartialEq)]
enum MiniBtn {
    Expand,
    Close,
    Pause,
}

/// Comment / reply composer.
struct Compose {
    uid: u64,
    video: String,
    /// (parent comment id, its index on the page)
    parent: Option<(String, usize)>,
    label: String,
    text: String,
    sending: bool,
    /// Sending to live chat rather than commenting.
    chat: bool,
    /// Naming a playlist instead of writing a comment.
    pl: Option<PlName>,
}

#[derive(Clone)]
enum PlName {
    /// New playlist, optionally adding this video to it.
    New(Option<VideoItem>),
    /// Rename (playlist id, privacy kept as is).
    Rename(String, String),
}

/// Modal list dialog ("Save to…", confirmations).
struct Dialog {
    title: String,
    rows: Vec<DRow>,
    sel: usize,
    video: Option<VideoItem>,
    /// playlist id -> Some(item id) when the video is in it; missing = unknown yet.
    member: std::collections::HashMap<String, Option<String>>,
    loading: bool,
}

#[derive(Clone)]
struct DRow {
    label: String,
    act: DAct,
}

#[derive(Clone)]
enum DAct {
    Later,
    Playlist(String),
    New,
    /// Confirmed: delete (playlist id, title).
    DeletePlaylist(String, String),
    Close,
}

pub enum Load {
    Feed(R<Feed>),
    More(R<Feed>),
    Channel(R<ChannelPage>),
    Playlist(R<PlaylistPage>),
    Watch(R<WatchInfo>),
    Comments(R<CommentBatch>),
    Replies(usize, R<CommentBatch>),
    Playlists(R<Vec<MyPlaylist>>),
    MyPlaylist(R<(MyPlaylist, Vec<PlItem>)>),
}

struct Tab {
    pages: Vec<Page>,
    idx: usize,
}

struct Edit {
    text: String,
    sugg: Vec<String>,
    sel: Option<usize>,
    new_tab: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Menu,
    Back,
    Fwd,
    Reload,
    Addr,
    Help,
}

#[derive(Clone)]
enum Side {
    Home,
    Subs,
    History,
    Later,
    Playlists,
    Help,
    Account,
    Channel(String),
}

#[derive(Default)]
struct Geo {
    page: Rect,
    tabs: Vec<(Rect, usize, bool)>,
    newtab: Rect,
    btns: Vec<(Rect, Btn)>,
    side: Vec<(Rect, usize)>,
    sugg: Vec<(Rect, usize)>,
    dlg: Vec<(Rect, usize)>,
    mini: Option<Rect>,
    mini_bar: Option<Rect>,
    mini_btns: Vec<(Rect, MiniBtn)>,
    fs_bar: Option<Rect>,
}

pub struct App {
    api: Arc<Api>,
    tx: Sender<Msg>,
    store: Store,
    store_rev: u64,
    renderer: Renderer,
    thumbs: Thumbs,
    tabs: Vec<Tab>,
    cur: usize,
    next_uid: u64,
    sidebar: bool,
    side_focus: Option<usize>,
    edit: Option<Edit>,
    player: Option<Player>,
    pending: Option<(VideoItem, f64)>,
    ended_handled: bool,
    /// Video id we already re-resolved after a stream failure.
    retried: Option<String>,
    chat: Option<Chat>,
    auth: Option<Arc<Auth>>,
    compose: Option<Compose>,
    dialog: Option<Dialog>,
    /// 3D audio visualiser instead of the video picture.
    viz: bool,
    engine3d: Engine,
    scenes: Scenes,
    analyzer: Analyzer,
    viz_cells: Vec<Cell>,
    viz_last: Instant,
    t0: Instant,
    /// Chat source for a video: (video id, token, is_replay).
    chat_src: Option<(String, String, bool)>,
    up_next: (String, Vec<VideoItem>),
    queue: Vec<VideoItem>,
    qpos: usize,
    fullscreen: bool,
    status: (String, Instant),
    opts: Opts,
    max_h: u32,
    dirty: bool,
    quit: bool,
    geo: Geo,
    last_record: Instant,
    term: (u16, u16),
}

fn s<T>(r: anyhow::Result<T>) -> R<T> {
    r.map_err(|e| e.to_string())
}

impl App {
    pub fn new(mut renderer: Renderer, opts: Opts, max_h: u32, mode_forced: bool) -> (App, Receiver<Msg>) {
        let (tx, rx) = channel();
        let txw = tx.clone();
        let thumbs = Thumbs::new(move || {
            let _ = txw.send(Msg::Wake);
        });
        let store = Store::load();
        if !mode_forced {
            if let Some(m) = store.mode.as_deref().and_then(Mode::parse) {
                renderer.mode = m;
            }
        }
        let app = App {
            api: Arc::new(Api::new()),
            tx,
            store,
            store_rev: 0,
            renderer,
            thumbs,
            tabs: Vec::new(),
            cur: 0,
            next_uid: 1,
            sidebar: true,
            side_focus: None,
            edit: None,
            player: None,
            pending: None,
            ended_handled: false,
            retried: None,
            chat: None,
            auth: Auth::load().map(Arc::new),
            compose: None,
            dialog: None,
            viz: false,
            engine3d: Engine::new(),
            scenes: Scenes::new(),
            analyzer: Analyzer::default(),
            viz_cells: Vec::new(),
            viz_last: Instant::now(),
            t0: Instant::now(),
            chat_src: None,
            up_next: (String::new(), Vec::new()),
            queue: Vec::new(),
            qpos: 0,
            fullscreen: false,
            status: (String::new(), Instant::now()),
            opts,
            max_h,
            dirty: true,
            quit: false,
            geo: Geo::default(),
            last_record: Instant::now(),
            term: (80, 24),
        };
        (app, rx)
    }

    // ------------------------------------------------------------ pages

    fn mk(&mut self, kind: Kind) -> Page {
        self.next_uid += 1;
        Page::new(self.next_uid, kind)
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.cur]
    }

    fn page(&self) -> &Page {
        let t = self.tab();
        &t.pages[t.idx]
    }

    fn page_mut(&mut self) -> &mut Page {
        let t = &mut self.tabs[self.cur];
        &mut t.pages[t.idx]
    }

    fn find(&mut self, uid: u64) -> Option<&mut Page> {
        self.tabs.iter_mut().flat_map(|t| t.pages.iter_mut()).find(|p| p.uid == uid)
    }

    pub fn open_tab(&mut self, kind: Kind) {
        let p = self.mk(kind);
        let uid = p.uid;
        self.tabs.push(Tab { pages: vec![p], idx: 0 });
        self.cur = self.tabs.len() - 1;
        self.load(uid);
    }

    pub fn navigate(&mut self, kind: Kind) {
        if self.tabs.is_empty() {
            return self.open_tab(kind);
        }
        let p = self.mk(kind);
        let uid = p.uid;
        let t = &mut self.tabs[self.cur];
        t.pages.truncate(t.idx + 1);
        t.pages.push(p);
        if t.pages.len() > 60 {
            t.pages.remove(0);
        }
        t.idx = t.pages.len() - 1;
        self.side_focus = None;
        self.load(uid);
    }

    fn back(&mut self) {
        let t = &mut self.tabs[self.cur];
        if t.idx > 0 {
            t.idx -= 1;
        }
    }

    fn forward(&mut self) {
        let t = &mut self.tabs[self.cur];
        if t.idx + 1 < t.pages.len() {
            t.idx += 1;
        }
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() == 1 {
            let home = self.mk(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
            let uid = home.uid;
            self.tabs[0] = Tab { pages: vec![home], idx: 0 };
            self.load(uid);
            return;
        }
        self.tabs.remove(i);
        if self.cur >= self.tabs.len() || self.cur > i {
            self.cur = self.cur.saturating_sub(1);
        }
    }

    pub fn default_chip(&self) -> usize {
        if self.store.history.is_empty() && self.store.subs.is_empty() {
            1
        } else {
            0
        }
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = (msg.into(), Instant::now());
    }

    fn store_changed(&mut self) {
        self.store.touch();
        self.store_rev += 1;
    }

    // ------------------------------------------------------------ loading

    fn load(&mut self, uid: u64) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        let hist: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            self.store.history.iter().map(|h| h.video.id.clone()).filter(|id| seen.insert(id.clone())).take(5).collect()
        };
        let watched: std::collections::HashSet<String> = self.store.history.iter().map(|h| h.video.id.clone()).collect();
        let subs: Vec<String> = self.store.subs.iter().map(|c| c.id.clone()).collect();
        let self_auth = self.auth.clone();
        let Some(p) = self.find(uid) else { return };
        p.loading = true;
        p.error = None;
        p.changed();
        let send = move |l: Load| {
            let _ = tx.send(Msg::Load(uid, l));
        };
        match &p.kind {
            Kind::Home { chip, .. } => {
                let chip = *chip;
                std::thread::spawn(move || {
                    if chip == 0 {
                        send(Load::Feed(Ok(for_you(&api, &hist, &subs, &watched))));
                    } else {
                        let (q, sp) = chip_query(CHIPS[chip]);
                        send(Load::Feed(s(api.search(q, sp))));
                    }
                });
            }
            Kind::Search { q, filter, .. } => {
                let (q, sp) = (q.clone(), FILTERS[*filter].1);
                std::thread::spawn(move || send(Load::Feed(s(api.search(&q, sp)))));
            }
            Kind::Channel { id, params, .. } => {
                let (id, params) = (id.clone(), params.clone());
                std::thread::spawn(move || send(Load::Channel(s(api.channel(&id, params.as_deref())))));
            }
            Kind::Playlist { id, .. } => {
                let id = id.clone();
                std::thread::spawn(move || send(Load::Playlist(s(api.playlist(&id)))));
            }
            Kind::Watch(w) => {
                let id = w.video.id.clone();
                std::thread::spawn(move || send(Load::Watch(s(api.watch(&id)))));
            }
            Kind::Subs { .. } => {
                std::thread::spawn(move || {
                    let entries = rss_all(&api, &subs).into_iter().take(90).map(|v| Entry::Item(Item::Video(v))).collect();
                    send(Load::Feed(Ok(Feed { entries, cont: None })));
                });
            }
            Kind::Playlists { .. } => match self_auth.clone() {
                Some(a) => {
                    std::thread::spawn(move || send(Load::Playlists(s(a.playlists()))));
                }
                None => p.loading = false,
            },
            Kind::MyPlaylist { id, .. } => match self_auth.clone() {
                Some(a) => {
                    let id = id.clone();
                    std::thread::spawn(move || send(Load::MyPlaylist(s(a.playlist_info(&id).and_then(|i| Ok((i, a.playlist_items(&id)?)))))));
                }
                None => {
                    p.loading = false;
                    p.error = Some("sign in to manage playlists".into());
                }
            },
            _ => p.loading = false,
        }
    }

    fn load_more(&mut self) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        let p = self.page_mut();
        let Some(tok) = p.more_token().map(String::from) else { return };
        if p.loading || p.more_loading {
            return;
        }
        p.more_loading = true;
        p.changed();
        let uid = p.uid;
        let send = move |l: Load| {
            let _ = tx.send(Msg::Load(uid, l));
        };
        match &mut p.kind {
            Kind::Home { .. } | Kind::Search { .. } => std::thread::spawn(move || send(Load::More(s(api.search_more(&tok))))),
            Kind::Channel { .. } | Kind::Playlist { .. } => std::thread::spawn(move || send(Load::More(s(api.browse_more(&tok))))),
            Kind::Watch(w) => {
                w.comments_loading = true;
                std::thread::spawn(move || send(Load::Comments(s(api.comments(&tok)))))
            }
            _ => return,
        };
    }

    fn on_load(&mut self, uid: u64, l: Load) {
        let mut want_comments = None;
        let mut want_rating: Option<String> = None;
        let mut related = None;
        let player_id = self.player.as_ref().map(|p| p.video.id.clone()).or(self.pending.as_ref().map(|p| p.0.id.clone()));
        let mut err_flash = None;
        let Some(p) = self.find(uid) else { return };
        p.loading = false;
        p.changed();
        match (l, &mut p.kind) {
            (Load::Feed(Ok(f)), Kind::Home { feed, .. } | Kind::Search { feed, .. } | Kind::Subs { feed }) => *feed = f,
            (Load::More(r), kind) => {
                p.more_loading = false;
                match (r, kind) {
                    (Ok(f), Kind::Home { feed, .. } | Kind::Search { feed, .. }) => feed.extend(f),
                    (Ok(f), Kind::Channel { page: Some(c), .. }) => c.feed.extend(f),
                    (Ok(f), Kind::Playlist { page: Some(pl), .. }) => pl.feed.extend(f),
                    (Err(e), _) => err_flash = Some(e),
                    _ => {}
                }
            }
            (Load::Channel(Ok(c)), Kind::Channel { page, .. }) => *page = Some(c),
            (Load::Playlists(Ok(l)), Kind::Playlists { list }) => *list = Some(l),
            (Load::MyPlaylist(Ok((i, it))), Kind::MyPlaylist { info, items, .. }) => {
                *info = Some(i);
                *items = it;
            }
            (Load::Playlists(Err(e)) | Load::MyPlaylist(Err(e)), _) => p.error = Some(e),
            (Load::Playlist(Ok(pl)), Kind::Playlist { page, .. }) => *page = Some(pl),
            (Load::Watch(Ok(info)), Kind::Watch(w)) => {
                if let Some(t) = &info.comments_token {
                    w.comments_loading = true;
                    want_comments = Some(t.clone());
                }
                want_rating = Some(w.video.id.clone());
                if w.video.title.is_empty() {
                    w.video.title = info.title.clone();
                    w.video.channel = info.channel.clone();
                    w.video.channel_id = Some(info.channel_id.clone());
                }
                if player_id.as_deref() == Some(w.video.id.as_str()) {
                    related = Some((w.video.id.clone(), info.related.videos(), w.video.clone(), info.chat.clone()));
                }
                w.info = Some(info);
            }
            (Load::Comments(r), Kind::Watch(w)) => {
                w.comments_loading = false;
                p.more_loading = false;
                match r {
                    Ok(b) => {
                        if b.count.is_some() {
                            w.comment_count = b.count;
                        }
                        w.comments.extend(b.comments);
                        w.comments_cont = b.cont;
                    }
                    Err(e) => {
                        w.comments_cont = None;
                        err_flash = Some(e);
                    }
                }
            }
            (Load::Replies(idx, r), Kind::Watch(w)) => match r {
                Ok(b) => {
                    if let Some(c) = w.comments.get_mut(idx) {
                        c.replies_token = None;
                    }
                    let at = (idx + 1).min(w.comments.len());
                    let replies: Vec<_> = b.comments.into_iter().map(|mut c| {
                        c.level = 1;
                        c
                    }).collect();
                    w.comments.splice(at..at, replies);
                }
                Err(e) => err_flash = Some(e),
            },
            (Load::Feed(Err(e)) | Load::Channel(Err(e)) | Load::Playlist(Err(e)) | Load::Watch(Err(e)), _) => p.error = Some(e),
            _ => {}
        }
        if let Some(e) = err_flash {
            self.flash(format!("⚠ {e}"));
        }
        if let (Some(id), Some(auth)) = (want_rating, self.auth.clone()) {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                if let Ok(r) = auth.rating(&id) {
                    let _ = tx.send(Msg::Rating(uid, r));
                }
            });
        }
        if let Some(tok) = want_comments {
            let (api, tx) = (self.api.clone(), self.tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Load(uid, Load::Comments(s(api.comments(&tok)))));
            });
        }
        if let Some((id, rel, v, chat)) = related {
            if let Some((tok, replay)) = chat {
                self.chat_src = Some((id.clone(), tok, replay));
            }
            self.up_next = (id, rel);
            if let Some(pl) = self.player.as_mut().filter(|p| p.video.id == v.id) {
                if pl.video.title.is_empty() {
                    pl.video.title = v.title;
                }
                if pl.video.channel.is_empty() {
                    pl.video.channel = v.channel;
                    pl.video.channel_id = v.channel_id;
                }
            }
        }
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Wake => {}
            Msg::Load(uid, l) => self.on_load(uid, l),
            Msg::Stream(id, r) => {
                let Some((v, start)) = self.pending.take_if(|p| p.0.id == id) else { return };
                match r {
                    Ok(st) => {
                        let mut v = v;
                        if v.title.is_empty() {
                            v.title = st.title.clone();
                        }
                        let dur = st.duration.unwrap_or(0.0);
                        self.store.record(&v, start, dur);
                        self.store_changed();
                        let (c, r) = self.term;
                        self.player = Some(Player::new(v, st, start, self.opts, (c as usize, r.saturating_sub(1) as usize), self.store.volume, self.store.exposure));
                        self.ended_handled = false;
                        self.last_record = Instant::now();
                        if start > 0.0 {
                            self.flash(format!("resuming at {}", fmt_time(start)));
                        }
                    }
                    Err(e) => self.flash(format!("⚠ can't play: {e}")),
                }
            }
            Msg::Suggest(q, list) => {
                if let Some(e) = &mut self.edit {
                    if e.text == q {
                        e.sugg = list;
                        e.sel = None;
                    }
                }
            }
            Msg::Related(id, rel, chat) => {
                if self.player.as_ref().is_some_and(|p| p.video.id == id) || self.pending.as_ref().is_some_and(|p| p.0.id == id) {
                    if let Some((tok, replay)) = chat {
                        self.chat_src = Some((id.clone(), tok, replay));
                    }
                    self.up_next = (id, rel);
                }
            }
            Msg::Rating(uid, r) => {
                if let Some(p) = self.find(uid) {
                    if let Kind::Watch(w) = &mut p.kind {
                        w.rating = Some(r);
                    }
                    p.changed();
                }
            }
            Msg::Rated(uid, prev, r) => match r {
                Ok(()) => {}
                Err(e) => {
                    if let Some(p) = self.find(uid) {
                        if let Kind::Watch(w) = &mut p.kind {
                            w.rating = Some(prev);
                        }
                        p.changed();
                    }
                    self.flash(format!("⚠ couldn't rate: {e}"));
                }
            },
            Msg::Posted(uid, parent, r) => match r {
                Ok(c) => {
                    self.compose = None;
                    if let Some(p) = self.find(uid) {
                        if let Kind::Watch(w) = &mut p.kind {
                            let at = match parent {
                                // After the parent thread's existing replies.
                                Some(i) => {
                                    let mut j = i + 1;
                                    while w.comments.get(j).is_some_and(|c| c.level > 0) {
                                        j += 1;
                                    }
                                    j.min(w.comments.len())
                                }
                                None => 0,
                            };
                            w.comments.insert(at, c);
                        }
                        p.changed();
                    }
                    self.flash(if parent.is_some() { "reply posted" } else { "comment posted" });
                }
                Err(e) => {
                    if let Some(c) = &mut self.compose {
                        c.sending = false;
                    }
                    self.flash(format!("⚠ couldn't post: {e}"));
                }
            },
            Msg::SubsSynced(r, manual) => match r {
                Ok(list) => {
                    let (added, removed) = self.store.merge_remote(&list);
                    self.store_changed();
                    if manual || added + removed > 0 {
                        self.flash(format!("subscriptions synced: {} on YouTube (+{added} −{removed})", list.len()));
                    }
                    if added + removed > 0 {
                        self.reload_subs_page();
                    }
                }
                Err(e) => self.flash(format!("⚠ couldn't sync subscriptions: {e}")),
            },
            Msg::SubDone(ch, name, on, r) => match r {
                Ok(sub_id) => {
                    if on {
                        self.store.set_sub_id(&ch, Some(sub_id).filter(|s| !s.is_empty()));
                    }
                    self.store_changed();
                }
                Err(e) if e.contains("already subscribed") => self.sync_subs(false),
                Err(e) => {
                    // Undo the optimistic local change.
                    if self.store.is_sub(&ch) == on {
                        self.store.toggle_sub(&ch, &name);
                    }
                    self.store_changed();
                    self.flash(format!("⚠ {}: {e}", if on { "couldn't subscribe" } else { "couldn't unsubscribe" }));
                }
            },
            Msg::ChatSent(r) => match r {
                Ok(()) => {
                    self.compose = None;
                    self.flash("message sent");
                }
                Err(e) => {
                    if let Some(c) = &mut self.compose {
                        c.sending = false;
                    }
                    self.flash(format!("⚠ couldn't send: {e}"));
                }
            },
            Msg::DlgPlaylists(vid, r) => {
                let Some(d) = self.dialog.as_mut().filter(|d| d.video.as_ref().is_some_and(|v| v.id == vid)) else { return };
                d.loading = false;
                match r {
                    Ok(list) => {
                        let keep_new = d.rows.pop();
                        for p in &list {
                            d.rows.push(DRow { label: format!("{} · {}", p.title, p.privacy), act: DAct::Playlist(p.id.clone()) });
                        }
                        d.rows.extend(keep_new);
                        if let Some(auth) = self.auth.clone() {
                            for p in list {
                                let (tx, a, v) = (self.tx.clone(), auth.clone(), vid.clone());
                                std::thread::spawn(move || {
                                    let found = a.find_item(&p.id, &v).ok().flatten();
                                    let _ = tx.send(Msg::DlgMember(v, p.id, found));
                                });
                            }
                        }
                    }
                    Err(e) => d.title = format!("Save to…  (⚠ {e})"),
                }
            }
            Msg::DlgMember(vid, pl, item) => {
                if let Some(d) = self.dialog.as_mut().filter(|d| d.video.as_ref().is_some_and(|v| v.id == vid)) {
                    d.member.insert(pl, item);
                }
            }
            Msg::DlgToggled(vid, pl, title, r) => match r {
                Ok(item) => {
                    let added = item.is_some();
                    if let Some(d) = self.dialog.as_mut().filter(|d| d.video.as_ref().is_some_and(|v| v.id == vid)) {
                        d.member.insert(pl, item);
                    }
                    self.flash(if added { format!("saved to {title}") } else { format!("removed from {title}") });
                    self.reload_playlist_pages();
                }
                Err(e) => {
                    if let Some(d) = self.dialog.as_mut() {
                        d.member.remove(&pl);
                    }
                    self.flash(format!("⚠ {e}"));
                }
            },
            Msg::PlDone(r) => {
                match r {
                    Ok(m) => {
                        self.compose = None;
                        self.flash(m);
                    }
                    Err(e) => {
                        if let Some(c) = &mut self.compose {
                            c.sending = false;
                        }
                        self.flash(format!("⚠ {e}"));
                    }
                }
                self.reload_playlist_pages();
            }
            Msg::Chat(id, u) => {
                if let Some(c) = self.chat.as_mut().filter(|c| c.video == id) {
                    c.apply(u);
                }
            }
            Msg::Open(r, new_tab) => match r {
                Ok(Target::Video(id)) => {
                    let v = VideoItem { id, ..Default::default() };
                    if new_tab {
                        self.open_tab(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
                    }
                    self.open_video(v, Vec::new(), 0);
                }
                Ok(t) => {
                    let kind = match t {
                        Target::Channel(id) => Kind::Channel { id, params: None, page: None },
                        Target::Playlist(id) => Kind::Playlist { id, page: None },
                        Target::Video(_) => unreachable!(),
                    };
                    if new_tab {
                        self.open_tab(kind)
                    } else {
                        self.navigate(kind)
                    }
                }
                Err(e) => self.flash(format!("⚠ {e}")),
            },
        }
        self.dirty = true;
    }

    // ------------------------------------------------------------ playback

    fn open_video(&mut self, v: VideoItem, queue: Vec<VideoItem>, qpos: usize) {
        let w = Watch {
            video: v.clone(),
            info: None,
            desc_open: false,
            comments: Vec::new(),
            comments_cont: None,
            comment_count: None,
            comments_loading: false,
            queue: queue.clone(),
            qpos,
            rating: None,
        };
        self.navigate(Kind::Watch(Box::new(w)));
        self.play(v, queue, qpos, None);
    }

    fn play(&mut self, v: VideoItem, queue: Vec<VideoItem>, qpos: usize, at: Option<f64>) {
        if self.player.as_ref().is_some_and(|p| p.video.id == v.id && !p.ended) {
            if let (Some(t), Some(p)) = (at, &mut self.player) {
                p.seek_to(t);
            }
            return;
        }
        self.stop();
        if !queue.is_empty() {
            self.queue = queue;
            self.qpos = qpos;
        }
        let start = at.unwrap_or_else(|| self.store.resume_pos(&v.id));
        self.pending = Some((v.clone(), start));
        let (api, tx, h) = (self.api.clone(), self.tx.clone(), self.max_h);
        let id = v.id.clone();
        std::thread::spawn(move || {
            let r = s(yt::resolve(&format!("https://www.youtube.com/watch?v={id}"), h));
            let _ = tx.send(Msg::Stream(id.clone(), r));
            if let Ok(w) = api.watch(&id) {
                let _ = tx.send(Msg::Related(id, w.related.videos(), w.chat));
            }
        });
    }

    fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            let d = p.duration();
            self.store.record(&p.video, if p.ended { d } else { p.pos }, d);
            self.store_changed();
        }
        self.pending = None;
        self.fullscreen = false;
        self.chat = None;
    }

    /// Next video: the playlist queue first, then "Up next".
    fn next_video(&self) -> Option<(VideoItem, usize)> {
        let cur = self.player.as_ref().map(|p| p.video.id.clone()).unwrap_or_default();
        if self.queue.iter().any(|v| v.id == cur) && self.qpos + 1 < self.queue.len() {
            return Some((self.queue[self.qpos + 1].clone(), self.qpos + 1));
        }
        if self.up_next.0 == cur {
            return self.up_next.1.iter().find(|v| v.id != cur && !v.live).cloned().map(|v| (v, usize::MAX));
        }
        None
    }

    fn advance(&mut self) {
        let Some((v, qpos)) = self.next_video() else {
            self.flash("nothing up next");
            return;
        };
        let cur = self.player.as_ref().map(|p| p.video.id.clone()).unwrap_or_default();
        let (queue, qpos) = if qpos == usize::MAX { (Vec::new(), 0) } else { (self.queue.clone(), qpos) };
        if qpos == 0 && queue.is_empty() {
            self.queue.clear();
        }
        let on_page = matches!(&self.page().kind, Kind::Watch(w) if w.video.id == cur);
        if on_page {
            self.open_video(v, queue, qpos);
        } else {
            self.play(v, queue, qpos, None);
        }
    }

    fn sync_chat(&mut self) {
        let Some(p) = &self.player else {
            self.chat = None;
            return;
        };
        let id = p.video.id.clone();
        if let Some(c) = &self.chat {
            if c.video == id {
                c.set_pos(p.pos);
                return;
            }
            self.chat = None;
        }
        if let Some((sid, tok, replay)) = self.chat_src.clone() {
            if sid == id {
                let tx = self.tx.clone();
                let pos = (p.pos * 1000.0) as u64;
                self.chat = Some(Chat::start(self.api.clone(), id, tok, replay, pos, move |vid, u| {
                    let _ = tx.send(Msg::Chat(vid, u));
                }));
            }
        }
    }

    fn after_tick(&mut self) {
        self.sync_chat();
        let Some(p) = &self.player else { return };
        if p.failed && self.retried.as_deref() != Some(p.video.id.as_str()) {
            // Stream URLs expire; fetch a fresh one once and carry on.
            let (v, pos) = (p.video.clone(), p.pos);
            self.retried = Some(v.id.clone());
            self.flash("stream dropped, reconnecting…");
            self.player = None;
            let (q, i) = (self.queue.clone(), self.qpos);
            self.play(v, q, i, Some(pos));
            self.dirty = true;
            return;
        }
        if p.ended && !self.ended_handled {
            self.ended_handled = true;
            let d = p.duration();
            let v = p.video.clone();
            self.store.record(&v, d, d);
            self.store_changed();
            if self.store.autoplay {
                self.advance();
            }
            self.dirty = true;
        } else if !p.paused && self.last_record.elapsed() > Duration::from_secs(5) {
            let (v, pos, d) = (p.video.clone(), p.pos, p.duration());
            self.store.record(&v, pos, d);
            self.store.touch();
            self.last_record = Instant::now();
        }
    }

    // ------------------------------------------------------------ actions

    fn change_volume(&mut self, by: f32) {
        let v = (self.store.volume + by).clamp(0.0, 1.5);
        let v = (v * 20.0).round() / 20.0;
        self.store.volume = v;
        self.store.touch();
        if let Some(p) = &mut self.player {
            p.set_volume(v);
        }
        let bar = "█".repeat((v * 10.0).round() as usize) + &"░".repeat(15 - (v * 10.0).round() as usize);
        self.flash(format!("volume {bar} {:.0}%", v * 100.0));
    }

    fn current_watch_video(&self) -> Option<(VideoItem, Vec<VideoItem>, usize)> {
        match &self.page().kind {
            Kind::Watch(w) => Some((w.video.clone(), w.queue.clone(), w.qpos)),
            _ => None,
        }
    }

    fn activate(&mut self, act: Act, click_x: Option<u16>, tgt_x: u16, tgt_w: u16) {
        match act {
            Act::Video(v) => self.open_video(v, Vec::new(), 0),
            Act::QueueVideo(list, i) => {
                let v = list[i].clone();
                self.open_video(v, list, i);
            }
            Act::Channel(id) if !id.is_empty() => self.navigate(Kind::Channel { id, params: None, page: None }),
            Act::Channel(_) => {}
            Act::Playlist(id) => self.navigate(Kind::Playlist { id, page: None }),
            Act::Chip(i) => {
                let p = self.page_mut();
                if let Kind::Home { chip, feed } = &mut p.kind {
                    *chip = i;
                    *feed = Feed::default();
                }
                p.scroll = 0;
                let uid = p.uid;
                self.load(uid);
            }
            Act::Filter(i) => {
                let p = self.page_mut();
                if let Kind::Search { filter, feed, .. } = &mut p.kind {
                    *filter = i;
                    *feed = Feed::default();
                }
                p.scroll = 0;
                let uid = p.uid;
                self.load(uid);
            }
            Act::ChannelTab(i) => {
                let p = self.page_mut();
                if let Kind::Channel { params, page: Some(c), .. } = &mut p.kind {
                    *params = c.tabs.get(i).map(|t| t.1.clone());
                    c.selected = i;
                    c.feed = Feed::default();
                }
                let uid = p.uid;
                self.load(uid);
            }
            Act::ToggleSub(id, name) => {
                let sub_id = self.store.sub_id(&id);
                let on = self.store.toggle_sub(&id, &name);
                self.store_changed();
                let remote = self.auth.clone().filter(|_| on || sub_id.is_some());
                match remote {
                    Some(auth) => {
                        let tx = self.tx.clone();
                        let (ch, nm) = (id.clone(), name.clone());
                        std::thread::spawn(move || {
                            let r = if on { auth.subscribe(&ch) } else { auth.unsubscribe(sub_id.as_deref().unwrap_or("")).map(|_| String::new()) };
                            let _ = tx.send(Msg::SubDone(ch, nm, on, s(r)));
                        });
                        self.flash(if on { format!("subscribed to {name} on YouTube") } else { format!("unsubscribed from {name} on YouTube") });
                    }
                    None => self.flash(if on { format!("subscribed to {name}") } else { format!("unsubscribed from {name}") }),
                }
            }
            Act::ToggleLater(v) => {
                let on = self.store.toggle_later(&v);
                self.store_changed();
                self.flash(if on { "saved to Watch later" } else { "removed from Watch later" });
            }
            Act::ExpandDesc => {
                let p = self.page_mut();
                if let Kind::Watch(w) = &mut p.kind {
                    w.desc_open = !w.desc_open;
                }
                p.changed();
            }
            Act::Replies(idx) => {
                let (api, tx) = (self.api.clone(), self.tx.clone());
                let p = self.page_mut();
                let uid = p.uid;
                if let Kind::Watch(w) = &mut p.kind {
                    if let Some(tok) = w.comments.get(idx).and_then(|c| c.replies_token.clone()) {
                        std::thread::spawn(move || {
                            let _ = tx.send(Msg::Load(uid, Load::Replies(idx, s(api.comments(&tok)))));
                        });
                    }
                }
            }
            Act::Seek(t) => {
                if let Some((v, q, i)) = self.current_watch_video() {
                    self.play(v, q, i, Some(t));
                }
            }
            Act::Player => {
                if let Some((v, q, i)) = self.current_watch_video() {
                    match &mut self.player {
                        Some(p) if p.video.id == v.id => p.toggle_pause(),
                        _ if self.pending.as_ref().is_some_and(|p| p.0.id == v.id) => {}
                        _ => self.play(v, q, i, None),
                    }
                }
            }
            Act::PlayerBar => {
                if let (Some(cx), Some((v, ..))) = (click_x, self.current_watch_video()) {
                    if let Some(p) = self.player.as_mut().filter(|p| p.video.id == v.id) {
                        let bw = tgt_w.saturating_sub(18).max(1);
                        let frac = (cx.saturating_sub(tgt_x) as f64 / bw as f64).clamp(0.0, 1.0);
                        p.seek_to(frac * p.duration());
                    }
                }
            }
            Act::PlayAll => {
                if let Kind::Playlist { page: Some(pl), .. } = &self.page().kind {
                    let vids = pl.feed.videos();
                    if let Some(v) = vids.first().cloned() {
                        self.open_video(v, vids, 0);
                    }
                }
            }
            Act::Rate(want) => {
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                let p = self.page_mut();
                let uid = p.uid;
                let Kind::Watch(w) = &mut p.kind else { return };
                let prev = w.rating.clone().unwrap_or_else(|| "none".into());
                let new = if prev == want { "none".to_string() } else { want };
                w.rating = Some(new.clone());
                let id = w.video.id.clone();
                p.changed();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Rated(uid, prev, s(auth.rate(&id, &new))));
                });
            }
            Act::Compose | Act::ReplyTo(_) => {
                if self.auth.is_none() {
                    return self.need_sign_in();
                }
                let p = self.page();
                let Kind::Watch(w) = &p.kind else { return };
                let title = w.info.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| w.video.title.clone());
                let (parent, label) = match act {
                    Act::ReplyTo(i) => {
                        // Replies to a reply go to the top-level thread.
                        let top = (0..=i).rev().find(|&j| w.comments[j].level == 0).unwrap_or(i);
                        let c = &w.comments[top];
                        let mention = if w.comments[i].level > 0 { format!("{} ", w.comments[i].author) } else { String::new() };
                        (Some((c.id.clone(), top, mention)), format!("Reply to {}", w.comments[i].author))
                    }
                    _ => (None, format!("Comment on “{}”", page::trunc(&title, 60))),
                };
                self.compose = Some(Compose {
                    uid: p.uid,
                    video: w.video.id.clone(),
                    text: parent.as_ref().map(|p| p.2.clone()).unwrap_or_default(),
                    parent: parent.map(|(id, i, _)| (id, i)),
                    label,
                    sending: false,
                    chat: false,
                    pl: None,
                });
            }
            Act::SyncSubs => self.sync_subs(true),
            Act::UploadSubs => {
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                let local = self.store.local_only();
                self.flash(format!("subscribing to {} channels on YouTube…", local.len()));
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    for c in local {
                        let r = auth.subscribe(&c.id);
                        let quota = matches!(&r, Err(e) if e.to_string().contains("quota"));
                        let _ = tx.send(Msg::SubDone(c.id, c.name, true, s(r)));
                        if quota {
                            break;
                        }
                    }
                });
            }
            Act::ChatSend => self.open_chat_compose(),
            Act::SaveTo(v) => self.open_save_dialog(v),
            Act::NewPlaylist => self.name_playlist(PlName::New(None)),
            Act::OpenMyPlaylist(id) => self.navigate(Kind::MyPlaylist { id, info: None, items: Vec::new() }),
            Act::PlPlayAll => {
                if let Kind::MyPlaylist { items, .. } = &self.page().kind {
                    let vids: Vec<VideoItem> = items.iter().map(|i| i.video.clone()).collect();
                    if let Some(v) = vids.first().cloned() {
                        self.open_video(v, vids, 0);
                    }
                }
            }
            Act::PlRename => {
                if let Kind::MyPlaylist { id, info: Some(i), .. } = &self.page().kind {
                    let (id, pv, title) = (id.clone(), i.privacy.clone(), i.title.clone());
                    self.name_playlist(PlName::Rename(id, pv));
                    if let Some(c) = &mut self.compose {
                        c.text = title;
                    }
                }
            }
            Act::PlPrivacy => {
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                if let Kind::MyPlaylist { id, info: Some(i), .. } = &self.page().kind {
                    let next = match i.privacy.as_str() {
                        "private" => "unlisted",
                        "unlisted" => "public",
                        _ => "private",
                    };
                    let (id, title, tx) = (id.clone(), i.title.clone(), self.tx.clone());
                    std::thread::spawn(move || {
                        let r = auth.update_playlist(&id, &title, next).map(|_| format!("“{title}” is now {next}"));
                        let _ = tx.send(Msg::PlDone(s(r)));
                    });
                }
            }
            Act::PlDelete => {
                if let Kind::MyPlaylist { id, info: Some(i), .. } = &self.page().kind {
                    let (id, title) = (id.clone(), i.title.clone());
                    self.dialog = Some(Dialog {
                        title: format!("Delete “{}”? This can't be undone.", page::trunc(&title, 50)),
                        rows: vec![
                            DRow { label: "Delete playlist".into(), act: DAct::DeletePlaylist(id, title) },
                            DRow { label: "Cancel".into(), act: DAct::Close },
                        ],
                        sel: 1,
                        video: None,
                        member: Default::default(),
                        loading: false,
                    });
                }
            }
            Act::PlMove(idx, delta) => {
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                if let Kind::MyPlaylist { id, items, .. } = &self.page().kind {
                    let Some(it) = items.get(idx).cloned() else { return };
                    let pos = (it.position as i64 + delta).clamp(0, items.len() as i64 - 1) as u64;
                    let (id, tx) = (id.clone(), self.tx.clone());
                    std::thread::spawn(move || {
                        let r = auth.move_item(&id, &it, pos).map(|_| format!("moved “{}”", it.video.title));
                        let _ = tx.send(Msg::PlDone(s(r)));
                    });
                }
            }
            Act::PlRemove(idx) => {
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                if let Kind::MyPlaylist { items, .. } = &self.page().kind {
                    let Some(it) = items.get(idx).cloned() else { return };
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let r = auth.remove_item(&it.item_id).map(|_| format!("removed “{}”", it.video.title));
                        let _ = tx.send(Msg::PlDone(s(r)));
                    });
                }
            }
            Act::SignOut => {
                if let Some(a) = self.auth.take() {
                    std::thread::spawn(move || a.sign_out());
                }
                self.store_rev += 1;
                self.flash("signed out");
            }
            Act::ClearHistory => {
                self.store.history.clear();
                self.store_changed();
                self.flash("history cleared");
            }
        }
    }

    fn reload_playlist_pages(&mut self) {
        let uids: Vec<u64> = self
            .tabs
            .iter()
            .flat_map(|t| t.pages.iter())
            .filter(|p| matches!(p.kind, Kind::Playlists { .. } | Kind::MyPlaylist { .. }))
            .map(|p| p.uid)
            .collect();
        for uid in uids {
            self.load(uid);
        }
    }

    fn open_save_dialog(&mut self, v: VideoItem) {
        let mut rows = vec![DRow { label: "Watch later (ytui)".into(), act: DAct::Later }];
        let signed_in = self.auth.is_some();
        rows.push(DRow { label: if signed_in { "+ New playlist".into() } else { "Sign in to save to your YouTube playlists".into() }, act: DAct::New });
        let title = format!("Save “{}” to…", page::trunc(&v.title, 50));
        self.dialog = Some(Dialog { title, rows, sel: 0, video: Some(v.clone()), member: Default::default(), loading: signed_in });
        if let Some(auth) = self.auth.clone() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::DlgPlaylists(v.id.clone(), s(auth.playlists())));
            });
        }
    }

    fn name_playlist(&mut self, what: PlName) {
        if self.auth.is_none() {
            return self.need_sign_in();
        }
        let label = match &what {
            PlName::New(Some(v)) => format!("New playlist (will add “{}”) · created private", page::trunc(&v.title, 40)),
            PlName::New(None) => "New playlist name · created private".into(),
            PlName::Rename(..) => "Rename playlist".into(),
        };
        self.compose = Some(Compose { uid: self.page().uid, video: String::new(), parent: None, label, text: String::new(), sending: false, chat: false, pl: Some(what) });
    }

    /// Activate dialog row `i`.
    fn dialog_pick(&mut self, i: usize) {
        let Some(d) = &self.dialog else { return };
        let Some(row) = d.rows.get(i).cloned() else { return };
        let video = d.video.clone();
        match row.act {
            DAct::Close => self.dialog = None,
            DAct::Later => {
                if let Some(v) = video {
                    self.activate(Act::ToggleLater(v), None, 0, 0);
                }
            }
            DAct::New => {
                self.dialog = None;
                if self.auth.is_none() {
                    return self.need_sign_in();
                }
                self.name_playlist(PlName::New(video));
            }
            DAct::Playlist(pl) => {
                let (Some(v), Some(auth)) = (video, self.auth.clone()) else { return };
                let Some(state) = self.dialog.as_ref().and_then(|d| d.member.get(&pl).cloned()) else {
                    self.flash("still checking that playlist…");
                    return;
                };
                let title = row.label.rsplit_once(" · ").map(|x| x.0.to_string()).unwrap_or(row.label.clone());
                if let Some(d) = &mut self.dialog {
                    d.member.remove(&pl); // shows "…" until the call returns
                }
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let r = match state {
                        Some(item) => auth.remove_item(&item).map(|_| None),
                        None => auth.add_to_playlist(&pl, &v.id).map(Some),
                    };
                    let _ = tx.send(Msg::DlgToggled(v.id, pl, title, s(r)));
                });
            }
            DAct::DeletePlaylist(id, title) => {
                self.dialog = None;
                let Some(auth) = self.auth.clone() else { return self.need_sign_in() };
                let tx = self.tx.clone();
                self.back();
                std::thread::spawn(move || {
                    let r = auth.delete_playlist(&id).map(|_| format!("deleted “{title}”"));
                    let _ = tx.send(Msg::PlDone(s(r)));
                });
            }
        }
    }

    fn sync_subs(&mut self, manual: bool) {
        let Some(auth) = self.auth.clone() else {
            if manual {
                self.need_sign_in();
            }
            return;
        };
        if manual {
            self.flash("syncing subscriptions…");
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = auth.subscriptions().map(|v| v.into_iter().map(|r| (r.channel_id, r.title, r.sub_id)).collect());
            let _ = tx.send(Msg::SubsSynced(s(r), manual));
        });
    }

    fn reload_subs_page(&mut self) {
        let uids: Vec<u64> = self.tabs.iter().flat_map(|t| t.pages.iter()).filter(|p| matches!(p.kind, Kind::Subs { .. })).map(|p| p.uid).collect();
        for uid in uids {
            self.load(uid);
        }
    }

    /// Composer for a live chat message on the playing (or shown) live stream.
    fn open_chat_compose(&mut self) {
        if self.auth.is_none() {
            return self.need_sign_in();
        }
        let live = self.chat.as_ref().filter(|c| !c.replay).map(|c| c.video.clone());
        let Some(video) = live else {
            self.flash("no live chat running (chat replays are read-only)");
            return;
        };
        let title = self.player.as_ref().map(|p| p.video.title.clone()).unwrap_or_default();
        self.compose = Some(Compose {
            uid: self.page().uid,
            video,
            parent: None,
            label: format!("Live chat · {}", page::trunc(&title, 50)),
            text: String::new(),
            sending: false,
            chat: true,
            pl: None,
        });
    }

    fn need_sign_in(&mut self) {
        self.flash("sign in to do that: see the Account page");
        self.navigate(Kind::Account);
    }

    fn post_compose(&mut self) {
        let (Some(c), Some(auth)) = (&mut self.compose, self.auth.clone()) else { return };
        let text = c.text.trim().to_string();
        if text.is_empty() || c.sending {
            return;
        }
        c.sending = true;
        if let Some(what) = c.pl.clone() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let r = match what {
                    PlName::New(v) => auth.create_playlist(&text, "private").and_then(|id| {
                        if let Some(v) = &v {
                            auth.add_to_playlist(&id, &v.id)?;
                        }
                        Ok(match v {
                            Some(v) => format!("created “{text}” and saved “{}”", v.title),
                            None => format!("created “{text}”"),
                        })
                    }),
                    PlName::Rename(id, pv) => auth.update_playlist(&id, &text, &pv).map(|_| format!("renamed to “{text}”")),
                };
                let _ = tx.send(Msg::PlDone(s(r)));
            });
            return;
        }
        if c.chat {
            let (video, tx) = (c.video.clone(), self.tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::ChatSent(s(auth.send_chat(&video, &text))));
            });
            return;
        }
        let (uid, video, parent, tx) = (c.uid, c.video.clone(), c.parent.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let (r, idx) = match parent {
                Some((pid, idx)) => (auth.reply(&pid, &text), Some(idx)),
                None => (auth.comment(&video, &text), None),
            };
            let _ = tx.send(Msg::Posted(uid, idx, s(r)));
        });
    }

    pub fn open_query(&mut self, q: &str) {
        self.go(q, false);
    }

    fn go(&mut self, text: &str, new_tab: bool) {
        let q = text.trim().to_string();
        if q.is_empty() {
            return;
        }
        if api::looks_like_url(&q) {
            let url = if q.starts_with('@') { format!("https://www.youtube.com/{q}") } else if q.starts_with("http") { q } else { format!("https://{q}") };
            let (api, tx) = (self.api.clone(), self.tx.clone());
            self.flash("opening…");
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Open(s(api.resolve_url(&url)), new_tab));
            });
        } else {
            let kind = Kind::Search { q, filter: 0, feed: Feed::default() };
            if new_tab {
                self.open_tab(kind)
            } else {
                self.navigate(kind)
            }
        }
    }

    fn reload(&mut self) {
        let p = self.page_mut();
        match &mut p.kind {
            Kind::Home { feed, .. } | Kind::Search { feed, .. } | Kind::Subs { feed } => *feed = Feed::default(),
            Kind::Channel { page, .. } => *page = None,
            Kind::Playlist { page, .. } => *page = None,
            Kind::Watch(w) => {
                w.info = None;
                w.comments.clear();
                w.comments_cont = None;
            }
            _ => {}
        }
        let uid = p.uid;
        self.load(uid);
    }

    fn side_items(&self) -> Vec<(String, Side)> {
        let mut v = vec![
            ("⌂  Home".to_string(), Side::Home),
            ("▤  Subscriptions".to_string(), Side::Subs),
            ("↺  History".to_string(), Side::History),
            ("◷  Watch later".to_string(), Side::Later),
            ("≡  Playlists".to_string(), Side::Playlists),
            ("?  Help".to_string(), Side::Help),
            (if self.auth.is_some() { "@  Account ✓".to_string() } else { "@  Sign in".to_string() }, Side::Account),
        ];
        for c in &self.store.subs {
            v.push((c.name.clone(), Side::Channel(c.id.clone())));
        }
        v
    }

    fn side_go(&mut self, i: usize) {
        let Some((_, item)) = self.side_items().get(i).cloned() else { return };
        let kind = match item {
            Side::Home => Kind::Home { chip: self.default_chip(), feed: Feed::default() },
            Side::Subs => Kind::Subs { feed: Feed::default() },
            Side::History => Kind::History,
            Side::Later => Kind::Later,
            Side::Playlists => Kind::Playlists { list: None },
            Side::Help => Kind::Help,
            Side::Account => Kind::Account,
            Side::Channel(id) => Kind::Channel { id, params: None, page: None },
        };
        self.navigate(kind);
    }

    fn start_edit(&mut self, new_tab: bool) {
        self.edit = Some(Edit { text: String::new(), sugg: Vec::new(), sel: None, new_tab });
    }

    fn suggest(&mut self) {
        let Some(e) = &self.edit else { return };
        let q = e.text.clone();
        if q.trim().is_empty() || api::looks_like_url(&q) {
            if let Some(e) = &mut self.edit {
                e.sugg.clear();
            }
            return;
        }
        let (api, tx) = (self.api.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let list = api.suggest(&q);
            let _ = tx.send(Msg::Suggest(q, list));
        });
    }

    // ------------------------------------------------------------ input

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Paste(t) => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                if let Some(c) = &mut self.compose {
                    c.text.push_str(&t);
                } else if let Some(e) = &mut self.edit {
                    e.text.push_str(t.lines().next().unwrap_or(""));
                    self.suggest();
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => self.click(m.column, m.row),
                MouseEventKind::ScrollDown => self.scroll_by(3),
                MouseEventKind::ScrollUp => self.scroll_by(-3),
                _ => {}
            },
            _ => {}
        }
    }

    fn scroll_by(&mut self, d: isize) {
        if self.fullscreen {
            return;
        }
        let p = self.page_mut();
        p.scroll = (p.scroll as isize + d).max(0) as usize;
    }

    fn doc(&self) -> Option<&Doc> {
        self.page().layout.as_ref().map(|l| &l.1)
    }

    fn click(&mut self, x: u16, y: u16) {
        let hit = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if self.fullscreen {
            if let (Some(b), Some(p)) = (self.geo.fs_bar, &mut self.player) {
                if hit(&b) {
                    let frac = (x - b.x) as f64 / b.width.max(1) as f64;
                    p.seek_to(frac * p.duration());
                    return;
                }
                p.toggle_pause();
            }
            return;
        }
        if self.dialog.is_some() {
            match self.geo.dlg.iter().find(|(r, _)| hit(r)).map(|x| x.1) {
                Some(i) => {
                    if let Some(d) = &mut self.dialog {
                        d.sel = i;
                    }
                    self.dialog_pick(i);
                }
                None => self.dialog = None,
            }
            return;
        }
        if let Some(e) = &self.edit {
            if let Some(&(_, i)) = self.geo.sugg.iter().find(|(r, _)| hit(r)) {
                let (q, nt) = (e.sugg[i].clone(), e.new_tab);
                self.edit = None;
                return self.go(&q, nt);
            }
            self.edit = None;
        }
        if let Some(&(_, i, close)) = self.geo.tabs.iter().find(|(r, ..)| hit(r)) {
            if close {
                self.close_tab(i);
            } else {
                self.cur = i;
            }
            return;
        }
        if hit(&self.geo.newtab) {
            self.open_tab(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
            return self.start_edit(false);
        }
        if let Some(&(_, b)) = self.geo.btns.iter().find(|(r, _)| hit(r)) {
            match b {
                Btn::Menu => self.sidebar = !self.sidebar,
                Btn::Back => self.back(),
                Btn::Fwd => self.forward(),
                Btn::Reload => self.reload(),
                Btn::Addr => self.start_edit(false),
                Btn::Help => self.navigate(Kind::Help),
            }
            return;
        }
        if let Some(&(_, b)) = self.geo.mini_btns.iter().find(|(r, _)| hit(r)) {
            match b {
                MiniBtn::Close => {
                    self.stop();
                    self.flash("player closed");
                }
                MiniBtn::Expand => self.return_to_player(),
                MiniBtn::Pause => {
                    if let Some(p) = &mut self.player {
                        p.toggle_pause();
                    }
                }
            }
            return;
        }
        if let Some(m) = self.geo.mini {
            if hit(&m) {
                if let (Some(b), Some(p)) = (self.geo.mini_bar, &mut self.player) {
                    if hit(&b) {
                        let frac = (x - b.x) as f64 / b.width.max(1) as f64;
                        p.seek_to(frac * p.duration());
                        return;
                    }
                }
                return self.return_to_player();
            }
        }
        if let Some(&(_, i)) = self.geo.side.iter().find(|(r, _)| hit(r)) {
            return self.side_go(i);
        }
        let pr = self.geo.page;
        if hit(&pr) {
            let (dx, dy) = (x - pr.x, (y - pr.y) as usize + self.page().scroll);
            let found = self.doc().and_then(|d| {
                d.targets.iter().enumerate().find(|(_, t)| dx >= t.x && dx < t.x + t.w && dy >= t.y && dy < t.y + t.h as usize).map(|(i, t)| (i, t.clone()))
            });
            if let Some((i, t)) = found {
                self.page_mut().focus = Some(i);
                self.activate(t.act, Some(dx), t.x, t.w);
            }
        }
    }

    fn return_to_player(&mut self) {
        let Some(p) = &self.player else { return };
        let id = p.video.id.clone();
        if matches!(&self.page().kind, Kind::Watch(w) if w.video.id == id) {
            self.page_mut().scroll = 0;
        } else {
            let (v, q, i) = (p.video.clone(), self.queue.clone(), self.qpos);
            let keep = q.iter().any(|x| x.id == v.id);
            self.open_video(v, if keep { q } else { Vec::new() }, if keep { i } else { 0 });
        }
    }

    fn focused(&self) -> Option<Act> {
        let p = self.page();
        let d = self.doc()?;
        p.focus.and_then(|i| d.targets.get(i)).map(|t| t.act.clone())
    }

    fn move_focus(&mut self, dir: (i32, i32)) {
        let vh = self.geo.page.height as usize;
        let p = self.page();
        let Some(d) = p.layout.as_ref().map(|l| &l.1) else { return };
        if d.targets.is_empty() {
            return self.scroll_by(dir.1 as isize * 3);
        }
        let scroll = p.scroll;
        let visible = |t: &page::Tgt| t.y + t.h as usize > scroll && t.y < scroll + vh;
        let cur = p.focus.filter(|&i| d.targets.get(i).is_some_and(visible));
        let next = match cur {
            None => {
                let mut v: Vec<usize> = (0..d.targets.len()).filter(|&i| visible(&d.targets[i])).collect();
                if dir.1 < 0 || dir.0 < 0 {
                    v.reverse();
                }
                v.first().copied().or(Some(0))
            }
            Some(c) => {
                let r = &d.targets[c];
                let (rx0, rx1, ry0, ry1) = (r.x as i64, (r.x + r.w) as i64, r.y as i64, r.y as i64 + r.h as i64);
                // Gap between ranges (0 when they overlap).
                let gap = |a0: i64, a1: i64, b0: i64, b1: i64| (b0 - a1).max(a0 - b1).max(0);
                let pick = |strict: bool| {
                    let mut best: Option<(i64, usize)> = None;
                    for (i, t) in d.targets.iter().enumerate() {
                        if i == c {
                            continue;
                        }
                        let (tx0, tx1, ty0, ty1) = (t.x as i64, (t.x + t.w) as i64, t.y as i64, t.y as i64 + t.h as i64);
                        let hx = gap(rx0, rx1, tx0, tx1);
                        let vy = gap(ry0, ry1, ty0, ty1);
                        let score = match dir {
                            (0, 1) if (strict && ty0 >= ry1) || (!strict && ty0 > ry0) => Some((ty0 - ry1).max(0) * 4 + hx * 2 + (tx0 - rx0).abs() / 4),
                            (0, -1) if (strict && ty1 <= ry0) || (!strict && ty0 < ry0) => Some((ry0 - ty1).max(0) * 4 + hx * 2 + (tx0 - rx0).abs() / 4),
                            (1, 0) if tx0 >= rx1 && vy == 0 => Some((tx0 - rx1) + (ty0 - ry0).abs() * 2),
                            (-1, 0) if tx1 <= rx0 && vy == 0 => Some((rx0 - tx1) + (ty0 - ry0).abs() * 2),
                            _ => None,
                        };
                        if let Some(sc) = score {
                            if best.is_none_or(|b| sc < b.0) {
                                best = Some((sc, i));
                            }
                        }
                    }
                    best
                };
                let best = pick(true).or_else(|| pick(false));
                match best {
                    Some((_, i)) => Some(i),
                    None if dir.1 != 0 => {
                        let p = self.page_mut();
                        p.scroll = (p.scroll as isize + dir.1 as isize * 3).max(0) as usize;
                        return;
                    }
                    None => Some(c),
                }
            }
        };
        let Some(i) = next else { return };
        let t = d.targets[i].clone();
        let p = self.page_mut();
        p.focus = Some(i);
        if t.y < p.scroll {
            p.scroll = t.y.saturating_sub(1);
        } else if t.y + t.h as usize > p.scroll + vh {
            p.scroll = if t.h as usize >= vh { t.y } else { t.y + t.h as usize + 1 - vh };
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // Dialog
        if let Some(d) = &mut self.dialog {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.dialog = None,
                KeyCode::Up | KeyCode::Char('k') => d.sel = d.sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => d.sel = (d.sel + 1).min(d.rows.len().saturating_sub(1)),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let i = d.sel;
                    self.dialog_pick(i);
                }
                _ => {}
            }
            return;
        }
        // Comment composer
        if let Some(c) = &mut self.compose {
            if c.sending {
                return;
            }
            match k.code {
                KeyCode::Esc => self.compose = None,
                KeyCode::Enter if alt || k.modifiers.contains(KeyModifiers::SHIFT) => c.text.push('\n'),
                KeyCode::Enter => self.post_compose(),
                KeyCode::Backspace => {
                    c.text.pop();
                }
                KeyCode::Char('u') if ctrl => c.text.clear(),
                KeyCode::Char(ch) if !ctrl => c.text.push(ch),
                _ => {}
            }
            return;
        }
        // Address bar editing
        if let Some(e) = &mut self.edit {
            let mut changed = false;
            match k.code {
                KeyCode::Esc => self.edit = None,
                KeyCode::Enter => {
                    let q = e.sel.and_then(|i| e.sugg.get(i).cloned()).unwrap_or_else(|| e.text.clone());
                    let nt = e.new_tab;
                    self.edit = None;
                    self.go(&q, nt);
                }
                KeyCode::Down if !e.sugg.is_empty() => e.sel = Some(e.sel.map_or(0, |i| (i + 1).min(e.sugg.len() - 1))),
                KeyCode::Up => e.sel = e.sel.and_then(|i| i.checked_sub(1)),
                KeyCode::Tab => {
                    if let Some(s) = e.sel.and_then(|i| e.sugg.get(i)).cloned().or_else(|| e.sugg.first().cloned()) {
                        e.text = s;
                        changed = true;
                    }
                }
                KeyCode::Backspace => {
                    if ctrl || alt {
                        let t = e.text.trim_end().to_string();
                        e.text = t.rsplit_once(' ').map(|(a, _)| format!("{a} ")).unwrap_or_default();
                    } else {
                        e.text.pop();
                    }
                    changed = true;
                }
                KeyCode::Char('u') if ctrl => {
                    e.text.clear();
                    changed = true;
                }
                KeyCode::Char('w') if ctrl => {
                    let t = e.text.trim_end().to_string();
                    e.text = t.rsplit_once(' ').map(|(a, _)| format!("{a} ")).unwrap_or_default();
                    changed = true;
                }
                KeyCode::Char(c) if !ctrl => {
                    e.text.push(c);
                    changed = true;
                }
                _ => {}
            }
            if changed {
                self.suggest();
            }
            return;
        }
        // Fullscreen player
        if self.fullscreen {
            if let Some(p) = &mut self.player {
                match k.code {
                    KeyCode::Esc | KeyCode::Char('f') | KeyCode::Char('q') => self.fullscreen = false,
                    KeyCode::Left | KeyCode::Char('h') => p.seek_by(-10.0),
                    KeyCode::Right | KeyCode::Char('l') => p.seek_by(10.0),
                    KeyCode::Down | KeyCode::Char('j') => p.seek_by(-60.0),
                    KeyCode::Up | KeyCode::Char('k') => p.seek_by(60.0),
                    _ => self.global_key(k),
                }
                return;
            }
            self.fullscreen = false;
        }
        // Sidebar focus
        if let Some(i) = self.side_focus {
            let n = self.side_items().len();
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.side_focus = Some(i.saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => self.side_focus = Some((i + 1).min(n.saturating_sub(1))),
                KeyCode::Enter => self.side_go(i),
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') | KeyCode::Esc => self.side_focus = None,
                _ => self.global_key(k),
            }
            return;
        }
        let vh = self.geo.page.height.max(4) as usize;
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_focus((0, -1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_focus((0, 1)),
            KeyCode::Left | KeyCode::Char('h') if !alt => self.move_focus((-1, 0)),
            KeyCode::Right | KeyCode::Char('l') if !alt => self.move_focus((1, 0)),
            KeyCode::Enter => {
                let tgt = {
                    let p = self.page();
                    p.focus.and_then(|i| self.doc().and_then(|d| d.targets.get(i).cloned()))
                };
                if let Some(t) = tgt {
                    self.activate(t.act, None, t.x, t.w);
                }
            }
            KeyCode::PageDown => self.scroll_by(vh as isize - 2),
            KeyCode::PageUp => self.scroll_by(-(vh as isize - 2)),
            KeyCode::Home | KeyCode::Char('g') => {
                let p = self.page_mut();
                p.scroll = 0;
                p.focus = None;
            }
            KeyCode::End | KeyCode::Char('G') => {
                let h = self.doc().map(|d| d.h).unwrap_or(0);
                self.page_mut().scroll = h;
            }
            KeyCode::Tab if self.sidebar => self.side_focus = Some(0),
            _ => self.global_key(k),
        }
    }

    fn global_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => self.start_edit(false),
            KeyCode::Char('l') if ctrl => self.start_edit(false),
            KeyCode::Char('t') if ctrl => {
                self.open_tab(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
                self.start_edit(false);
            }
            KeyCode::Char('w') if ctrl => self.close_tab(self.cur),
            KeyCode::Char('b') if ctrl => self.sidebar = !self.sidebar,
            KeyCode::Char('J') => self.cur = (self.cur + 1) % self.tabs.len(),
            KeyCode::Char('K') => self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len(),
            KeyCode::PageDown if ctrl => self.cur = (self.cur + 1) % self.tabs.len(),
            KeyCode::PageUp if ctrl => self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len(),
            KeyCode::Char('H') | KeyCode::Backspace => self.back(),
            KeyCode::Left if alt => self.back(),
            KeyCode::Char('L') => self.forward(),
            KeyCode::Right if alt => self.forward(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('?') => self.navigate(Kind::Help),
            KeyCode::Char('m') => {
                self.renderer.mode = self.renderer.mode.next();
                self.store.mode = Some(self.renderer.mode.name().into());
                self.store.touch();
                self.flash(format!("render mode: {}", self.renderer.mode.name()));
            }
            KeyCode::Char('B') => self.renderer.ascii_bg = !self.renderer.ascii_bg,
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(0.05),
            KeyCode::Char('-') | KeyCode::Char('_') => self.change_volume(-0.05),
            KeyCode::Char('M') => {
                if let Some(p) = &mut self.player {
                    p.toggle_mute();
                    let msg = if p.muted() { "muted".to_string() } else { format!("volume {:.0}%", p.level() * 100.0) };
                    self.flash(msg);
                }
            }
            KeyCode::Char('I') => self.open_chat_compose(),
            KeyCode::Char('E') => {
                self.store.exposure = !self.store.exposure;
                self.store.touch();
                if let Some(p) = &mut self.player {
                    p.exposure.on = self.store.exposure;
                    p.exposure.reset();
                }
                self.flash(if self.store.exposure { "auto-exposure on" } else { "auto-exposure off" });
            }
            KeyCode::Char('V') => {
                self.viz = !self.viz;
                self.flash(if self.viz { "3D visualiser on (V to switch back to video)" } else { "video" });
            }
            KeyCode::Char('P') => {
                let v = match self.focused() {
                    Some(Act::Video(v)) => Some(v),
                    Some(Act::QueueVideo(l, i)) => Some(l[i].clone()),
                    _ => self.current_watch_video().map(|x| x.0).or(self.player.as_ref().map(|p| p.video.clone())),
                };
                if let Some(v) = v {
                    self.open_save_dialog(v);
                }
            }
            KeyCode::Char('C') => {
                self.store.show_chat = !self.store.show_chat;
                self.store_changed();
                self.flash(if self.store.show_chat { "chat shown" } else { "chat hidden" });
            }
            KeyCode::Char('[') => self.renderer.glob_exp = (self.renderer.glob_exp - 0.2).max(1.0),
            KeyCode::Char(']') => self.renderer.glob_exp += 0.2,
            KeyCode::Char('{') => self.renderer.dir_exp = (self.renderer.dir_exp - 0.25).max(1.0),
            KeyCode::Char('}') => self.renderer.dir_exp += 0.25,
            KeyCode::Char('a') => {
                self.store.autoplay = !self.store.autoplay;
                self.store_changed();
                self.flash(format!("autoplay {}", if self.store.autoplay { "on" } else { "off" }));
            }
            KeyCode::Char(' ') => match &mut self.player {
                Some(p) => p.toggle_pause(),
                None => {
                    if let Some((v, q, i)) = self.current_watch_video() {
                        self.play(v, q, i, None);
                    }
                }
            },
            KeyCode::Char('f') if self.player.is_some() => self.fullscreen = true,
            KeyCode::Char('x') if self.player.is_some() || self.pending.is_some() => {
                self.stop();
                self.flash("player closed");
            }
            KeyCode::Char('n') => self.advance(),
            KeyCode::Char(',') => self.player.iter_mut().for_each(|p| p.seek_by(-10.0)),
            KeyCode::Char('.') => self.player.iter_mut().for_each(|p| p.seek_by(10.0)),
            KeyCode::Char('<') => self.player.iter_mut().for_each(|p| p.seek_by(-60.0)),
            KeyCode::Char('>') => self.player.iter_mut().for_each(|p| p.seek_by(60.0)),
            KeyCode::Char(c @ '0'..='9') => {
                if let Some(p) = &mut self.player {
                    let d = p.duration();
                    if d > 0.0 {
                        p.seek_to(d * (c as u8 - b'0') as f64 / 10.0);
                    }
                }
            }
            KeyCode::Char('w') => {
                let focused = match self.focused() {
                    Some(Act::Video(v)) => Some(v),
                    Some(Act::QueueVideo(l, i)) => Some(l[i].clone()),
                    _ => None,
                };
                if let Some(v) = focused {
                    self.activate(Act::ToggleLater(v), None, 0, 0);
                } else if let Some((v, ..)) = self.current_watch_video() {
                    self.activate(Act::ToggleLater(v), None, 0, 0);
                }
            }
            KeyCode::Char('c') => {
                let cid = match self.focused() {
                    Some(Act::Video(v)) => v.channel_id,
                    Some(Act::QueueVideo(l, i)) => l[i].channel_id.clone(),
                    _ => None,
                };
                if let Some(id) = cid {
                    self.navigate(Kind::Channel { id, params: None, page: None });
                }
            }
            KeyCode::Char('t') => {
                let kind = match self.focused() {
                    Some(Act::Video(v)) => Some(v),
                    Some(Act::QueueVideo(l, i)) => Some(l[i].clone()),
                    Some(Act::Channel(id)) => {
                        self.open_tab(Kind::Channel { id, params: None, page: None });
                        None
                    }
                    Some(Act::Playlist(id)) => {
                        self.open_tab(Kind::Playlist { id, page: None });
                        None
                    }
                    _ => None,
                };
                if let Some(v) = kind {
                    self.open_tab(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
                    let w = Watch { video: v, info: None, desc_open: false, comments: Vec::new(), comments_cont: None, comment_count: None, comments_loading: false, queue: Vec::new(), qpos: 0, rating: None };
                    self.navigate(Kind::Watch(Box::new(w)));
                }
            }
            KeyCode::Char('s') => {
                let target = match &self.page().kind {
                    Kind::Watch(w) => w.info.as_ref().map(|i| (i.channel_id.clone(), i.channel.clone())),
                    Kind::Channel { id, page, .. } => Some((id.clone(), page.as_ref().map(|p| p.name.clone()).unwrap_or_default())),
                    _ => None,
                };
                if let Some((id, name)) = target {
                    self.activate(Act::ToggleSub(id, name), None, 0, 0);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.geo = Geo::default();
        self.term = (area.width, area.height);
        if let Some(p) = &mut self.player {
            p.set_term(area.width as usize, area.height.saturating_sub(1) as usize);
        }
        if area.height < 8 || area.width < 30 {
            f.buffer_mut().set_string(0, 0, "window too small", Style::default());
            return;
        }
        if self.fullscreen && self.player.is_some() {
            return self.draw_fullscreen(f.buffer_mut(), area);
        }
        let buf = f.buffer_mut();
        let bg = Style::default().bg(Color::Rgb(15, 15, 18)).fg(Color::White);
        buf.set_style(area, bg);
        self.draw_tabs(buf, Rect { height: 1, ..area });
        self.draw_addr(buf, Rect { y: area.y + 1, height: 1, ..area });
        let body = Rect { y: area.y + 2, height: area.height - 3, ..area };
        let show_side = self.sidebar && area.width >= 80;
        let side_w = if show_side { 24 } else { 0 };
        if show_side {
            self.draw_side(buf, Rect { width: side_w, ..body });
        }
        let pr = Rect { x: body.x + side_w + 1, width: body.width - side_w - 1, ..body };
        self.geo.page = pr;
        let player_visible = self.draw_page(buf, pr);
        if self.player.is_some() && !player_visible {
            self.draw_mini(buf, pr);
        }
        self.draw_status(buf, Rect { y: area.y + area.height - 1, height: 1, ..area });
        if self.edit.is_some() {
            self.draw_suggestions(buf);
        }
        if self.compose.is_some() {
            self.draw_compose(buf, area);
        }
        if self.dialog.is_some() {
            self.draw_dialog(buf, area);
        }
    }

    fn draw_dialog(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(d) = &self.dialog else { return };
        let later = d.video.as_ref().is_some_and(|v| self.store.later.iter().any(|x| x.id == v.id));
        let w = area.width.saturating_sub(8).min(70);
        let rows = d.rows.len().min(area.height as usize - 8);
        let h = rows as u16 + 4 + d.loading as u16;
        let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + area.height.saturating_sub(h) / 2, width: w, height: h };
        fill(buf, area, r.x, (r.y - area.y) as i64, w, h, Color::Rgb(30, 30, 38));
        let border = Style::default().fg(ACCENT);
        for x in r.x..r.x + w {
            buf[(x, r.y)].set_char('─').set_style(border);
            buf[(x, r.y + h - 1)].set_char('─').set_style(border);
        }
        buf.set_string(r.x + 2, r.y, format!(" {} ", page::trunc(&d.title, w as usize - 6)), Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        let mut y = r.y + 2;
        let first = d.sel.saturating_sub(rows.saturating_sub(1));
        for (i, row) in d.rows.iter().enumerate().skip(first).take(rows) {
            let mark = match &row.act {
                DAct::Later => if later { "[✓] " } else { "[ ] " }.to_string(),
                DAct::Playlist(id) => match d.member.get(id) {
                    Some(Some(_)) => "[✓] ".into(),
                    Some(None) => "[ ] ".into(),
                    None => "[…] ".into(),
                },
                _ => "    ".into(),
            };
            let sel = i == d.sel;
            let style = if sel { Style::default().fg(Color::White).bg(ACCENT) } else { Style::default().fg(Color::Rgb(225, 225, 225)) };
            let line = Rect { x: r.x + 1, y, width: w - 2, height: 1 };
            if sel {
                buf.set_style(line, style);
            }
            buf.set_string(r.x + 2, y, page::trunc(&format!("{mark}{}", row.label), w as usize - 4), style);
            self.geo.dlg.push((line, i));
            y += 1;
        }
        if d.loading {
            buf.set_string(r.x + 2, y, "loading your playlists…", Style::default().fg(DIM));
        }
        buf.set_string(r.x + 2, r.y + h - 2, "↑↓ choose · Enter toggle · Esc close", Style::default().fg(DIM));
    }

    fn draw_compose(&self, buf: &mut Buffer, area: Rect) {
        let Some(c) = &self.compose else { return };
        let w = area.width.saturating_sub(8).min(90);
        let tw = (w - 4) as usize;
        let lines = page::wrap(&format!("{}▏", c.text), tw, 200);
        let shown = lines.len().clamp(3, 10);
        let h = shown as u16 + 5;
        let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + area.height.saturating_sub(h) / 2, width: w, height: h };
        fill(buf, area, r.x, (r.y - area.y) as i64, w, h, Color::Rgb(30, 30, 38));
        let border = Style::default().fg(ACCENT);
        for x in r.x..r.x + w {
            buf[(x, r.y)].set_char('─').set_style(border);
            buf[(x, r.y + h - 1)].set_char('─').set_style(border);
        }
        buf.set_string(r.x + 2, r.y, format!(" {} ", page::trunc(&c.label, tw - 2)), Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        let start = lines.len().saturating_sub(shown);
        for (i, l) in lines[start..].iter().enumerate() {
            buf.set_string(r.x + 2, r.y + 2 + i as u16, l, Style::default().fg(Color::White));
        }
        let hint = if c.sending { "posting…".to_string() } else { format!("Enter post · Alt-Enter new line · Esc cancel · {} chars", c.text.chars().count()) };
        buf.set_string(r.x + 2, r.y + h - 2, page::trunc(&hint, tw), Style::default().fg(DIM));
    }

    fn draw_tabs(&mut self, buf: &mut Buffer, r: Rect) {
        buf.set_style(r, Style::default().bg(Color::Rgb(8, 8, 10)));
        buf.set_string(r.x, r.y, " ▶ ytui ", Style::default().fg(Color::White).bg(ACCENT).add_modifier(Modifier::BOLD));
        let mut x = r.x + 9;
        let n = self.tabs.len().max(1) as u16;
        let tw = ((r.width.saturating_sub(14)) / n).clamp(8, 28);
        for i in 0..self.tabs.len() {
            let title = {
                let t = &self.tabs[i];
                let p = &t.pages[t.idx];
                let icon = if matches!(&p.kind, Kind::Watch(w) if self.player.as_ref().is_some_and(|pl| pl.video.id == w.video.id)) { "♪ " } else { "" };
                format!("{icon}{}", p.title())
            };
            if x + tw + 3 > r.x + r.width {
                break;
            }
            let sel = i == self.cur;
            let style = if sel { Style::default().bg(Color::Rgb(40, 40, 48)).fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::default().bg(Color::Rgb(22, 22, 26)).fg(META) };
            let label = format!(" {:<w$}", page::trunc(&title, tw as usize - 3), w = tw as usize - 3);
            buf.set_string(x, r.y, &label, style);
            buf.set_string(x + tw - 2, r.y, "× ", style.fg(DIM));
            self.geo.tabs.push((Rect { x, y: r.y, width: tw - 2, height: 1 }, i, false));
            self.geo.tabs.push((Rect { x: x + tw - 2, y: r.y, width: 2, height: 1 }, i, true));
            x += tw + 1;
        }
        buf.set_string(x, r.y, " + ", Style::default().fg(Color::White).bg(Color::Rgb(22, 22, 26)));
        self.geo.newtab = Rect { x, y: r.y, width: 3, height: 1 };
    }

    fn draw_addr(&mut self, buf: &mut Buffer, r: Rect) {
        buf.set_style(r, Style::default().bg(Color::Rgb(28, 28, 34)));
        let t = self.tab();
        let can_back = t.idx > 0;
        let can_fwd = t.idx + 1 < t.pages.len();
        let btn = |on: bool| Style::default().bg(Color::Rgb(28, 28, 34)).fg(if on { Color::White } else { DIM });
        let mut x = r.x + 1;
        for (label, b, on) in [(" ≡ ", Btn::Menu, true), (" ◀ ", Btn::Back, can_back), (" ▶ ", Btn::Fwd, can_fwd), (" ↻ ", Btn::Reload, true)] {
            buf.set_string(x, r.y, label, btn(on));
            self.geo.btns.push((Rect { x, y: r.y, width: 3, height: 1 }, b));
            x += 4;
        }
        let aw = r.width.saturating_sub(x - r.x + 6);
        let ar = Rect { x, y: r.y, width: aw, height: 1 };
        let field = Style::default().bg(Color::Rgb(10, 10, 12));
        buf.set_style(ar, field);
        match &self.edit {
            Some(e) => {
                let shown = if e.text.is_empty() { "Search YouTube or paste a URL".to_string() } else { e.text.clone() };
                let st = if e.text.is_empty() { field.fg(DIM) } else { field.fg(Color::White) };
                let tail = page::trunc(&shown, aw.saturating_sub(4) as usize);
                buf.set_string(x + 2, r.y, &tail, st);
                let cx = x + 2 + if e.text.is_empty() { 0 } else { tail.width() as u16 };
                if cx < x + aw {
                    buf[(cx, r.y)].set_char('▏').set_fg(ACCENT);
                }
                buf.set_style(Rect { x, y: r.y, width: 1, height: 1 }, Style::default().bg(ACCENT));
            }
            None => {
                let addr = self.page().address();
                let (scheme, rest) = addr.split_once("://").map(|(a, b)| (format!("{a}://"), b.to_string())).unwrap_or((String::new(), addr.clone()));
                let rest = if scheme.is_empty() { rest } else { rest };
                let mut sx = x + 2;
                if !scheme.is_empty() {
                    buf.set_string(sx, r.y, &scheme, field.fg(DIM));
                    sx += scheme.width() as u16;
                }
                let (host, path) = rest.split_once('/').map(|(h, p)| (h.to_string(), format!("/{p}"))).unwrap_or((rest.clone(), String::new()));
                buf.set_string(sx, r.y, &host, field.fg(Color::White));
                sx += host.width() as u16;
                let room = (x + aw).saturating_sub(sx + 1) as usize;
                buf.set_string(sx, r.y, page::trunc(&path, room), field.fg(META));
            }
        }
        self.geo.btns.push((ar, Btn::Addr));
        let hx = x + aw + 2;
        buf.set_string(hx, r.y, " ? ", btn(true));
        self.geo.btns.push((Rect { x: hx, y: r.y, width: 3, height: 1 }, Btn::Help));
    }

    fn draw_side(&mut self, buf: &mut Buffer, r: Rect) {
        buf.set_style(r, Style::default().bg(Color::Rgb(20, 20, 24)));
        let items = self.side_items();
        let cur_kind = match &self.page().kind {
            Kind::Home { .. } => 0,
            Kind::Subs { .. } => 1,
            Kind::History => 2,
            Kind::Later => 3,
            Kind::Playlists { .. } | Kind::MyPlaylist { .. } => 4,
            Kind::Help => 5,
            Kind::Account => 6,
            _ => usize::MAX,
        };
        let cur_channel = match &self.page().kind {
            Kind::Channel { id, .. } => Some(id.clone()),
            _ => None,
        };
        let mut y = r.y + 1;
        for (i, (label, item)) in items.iter().enumerate() {
            if i == 7 {
                y += 1;
                if y < r.y + r.height {
                    buf.set_string(r.x + 2, y, "SUBSCRIPTIONS", Style::default().fg(DIM).add_modifier(Modifier::BOLD));
                }
                y += 1;
            }
            if y >= r.y + r.height {
                break;
            }
            let active = i == cur_kind || matches!(item, Side::Channel(id) if Some(id) == cur_channel.as_ref());
            let focused = self.side_focus == Some(i);
            let mut style = Style::default().fg(if active { Color::White } else { META });
            if active {
                style = style.bg(Color::Rgb(45, 45, 55)).add_modifier(Modifier::BOLD);
            }
            if focused {
                style = style.bg(ACCENT).fg(Color::White);
            }
            let row = Rect { x: r.x, y, width: r.width, height: 1 };
            buf.set_style(row, style);
            buf.set_string(r.x + 2, y, page::trunc(label, r.width as usize - 3), style);
            self.geo.side.push((row, i));
            y += 1;
        }
    }

    /// Draw the current page; returns whether the inline player is visible.
    fn draw_page(&mut self, buf: &mut Buffer, r: Rect) -> bool {
        let (w, vh) = (r.width, r.height);
        let key_store = self.store_rev;
        let autoplay = self.store.autoplay;
        let show_chat = self.store.show_chat;
        let t = &mut self.tabs[self.cur];
        let page = &mut t.pages[t.idx];
        let key = (w, vh, page.rev, key_store);
        if page.layout.as_ref().map(|l| l.0) != Some(key) {
            let ctx = Ctx { autoplay, chat: show_chat, account: self.auth.as_ref().map(|a| a.channel()) };
            let doc = layout(page, &self.store, w, vh, &ctx);
            page.layout = Some((key, doc));
        }
        let doc = &page.layout.as_ref().unwrap().1;
        let max_scroll = doc.h.saturating_sub(vh as usize);
        page.scroll = page.scroll.min(max_scroll);
        let scroll = page.scroll;
        let mut player_visible = false;
        let pending_id = self.pending.as_ref().map(|p| p.0.id.clone());
        for prim in &doc.prims {
            match prim {
                Prim::Text { x, y, line } => {
                    if *y >= scroll && *y < scroll + vh as usize {
                        buf.set_line(r.x + x, r.y + (y - scroll) as u16, line, w.saturating_sub(*x));
                    }
                }
                Prim::Fill { x, y, w: fw, h, color } => {
                    for yy in *y..y + *h as usize {
                        if yy >= scroll && yy < scroll + vh as usize {
                            buf.set_style(Rect { x: r.x + x, y: r.y + (yy - scroll) as u16, width: (*fw).min(w.saturating_sub(*x)), height: 1 }, Style::default().bg(*color));
                        }
                    }
                }
                Prim::Image { x, y, w: iw, h, url, badge, progress } => {
                    if y + *h as usize <= scroll || *y >= scroll + vh as usize {
                        continue;
                    }
                    let iw = (*iw).min(w.saturating_sub(*x));
                    match self.thumbs.cells(url, iw, *h, &mut self.renderer) {
                        Some(cells) => blit(buf, r, r.x + x, *y as i64 - scroll as i64, cells, iw as usize, *h as usize),
                        None => fill(buf, r, r.x + x, *y as i64 - scroll as i64, iw, *h, Color::Rgb(38, 38, 46)),
                    }
                    let by = *y as i64 + *h as i64 - 1 - scroll as i64;
                    if let Some(p) = progress {
                        let n = ((iw as f32) * p).round() as u16;
                        for i in 0..iw {
                            if by >= 0 && (by as u16) < vh {
                                let c = &mut buf[(r.x + x + i, r.y + by as u16)];
                                c.set_char('▄').set_fg(if i < n { Color::Rgb(255, 0, 0) } else { Color::Rgb(90, 90, 90) });
                            }
                        }
                    }
                    if let Some((text, col)) = badge {
                        let bw = text.width() as u16;
                        let by = if progress.is_some() { by - 1 } else { by };
                        if bw + 1 < iw && by >= 0 && (by as u16) < vh {
                            buf.set_string(r.x + x + iw - bw - 1, r.y + by as u16, text, Style::default().fg(Color::White).bg(*col).add_modifier(Modifier::BOLD));
                        }
                    }
                }
                Prim::Player { x, y, w: pw, h, id } => {
                    let sy = *y as i64 - scroll as i64;
                    if sy + *h as i64 <= 0 || sy >= vh as i64 {
                        continue;
                    }
                    let pw = (*pw).min(w.saturating_sub(*x));
                    fill(buf, r, r.x + x, sy, pw, *h, Color::Black);
                    let mine = self.player.as_mut().filter(|p| &p.video.id == id);
                    match mine {
                        Some(p) => {
                            player_visible = sy + (*h as i64) / 2 >= 0 && sy + (*h as i64) / 2 < vh as i64;
                            if self.viz {
                                let t = self.t0.elapsed().as_secs_f32();
                                let samples = p.samples();
                                render_viz(&mut self.engine3d, &self.scenes, &mut self.analyzer, &mut self.renderer, &samples, SceneKind::Visualizer, t, &mut self.viz_last, pw as usize, *h as usize, &mut self.viz_cells);
                                blit(buf, r, r.x + x, sy, &self.viz_cells, pw as usize, *h as usize);
                            } else {
                                match p.cells(&mut self.renderer, pw as usize, *h as usize) {
                                    Some((cells, cw, ch, ox, oy)) => blit(buf, r, r.x + x + ox as u16, sy + oy as i64, cells, cw, ch),
                                    None => {
                                        // Connecting: keep the 3D logo spinning until the first frame.
                                        let t = self.t0.elapsed().as_secs_f32();
                                        render_viz(&mut self.engine3d, &self.scenes, &mut self.analyzer, &mut self.renderer, &[], SceneKind::Logo, t, &mut self.viz_last, pw as usize, *h as usize, &mut self.viz_cells);
                                        blit(buf, r, r.x + x, sy, &self.viz_cells, pw as usize, *h as usize);
                                        let msg = " Loading video… ";
                                        let y = sy + *h as i64 - 2;
                                        if y >= 0 && y < vh as i64 {
                                            buf.set_string(r.x + x + pw.saturating_sub(msg.chars().count() as u16) / 2, r.y + y as u16, msg, Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
                                        }
                                    }
                                }
                            }
                            if p.paused && !p.ended {
                                center_text(buf, r, r.x + x, sy, pw, *h, " ❚❚  paused ");
                            } else if p.ended {
                                center_text(buf, r, r.x + x, sy, pw, *h, " ↻  replay (space) ");
                            }
                        }
                        None => {
                            let loading = pending_id.as_deref() == Some(id.as_str());
                            if loading {
                                // Spinning 3D logo while the stream resolves.
                                let t = self.t0.elapsed().as_secs_f32();
                                render_viz(&mut self.engine3d, &self.scenes, &mut self.analyzer, &mut self.renderer, &[], SceneKind::Logo, t, &mut self.viz_last, pw as usize, *h as usize, &mut self.viz_cells);
                                blit(buf, r, r.x + x, sy, &self.viz_cells, pw as usize, *h as usize);
                                let msg = " Loading video… ";
                                let y = sy + *h as i64 - 2;
                                if y >= 0 && y < vh as i64 {
                                    buf.set_string(r.x + x + pw.saturating_sub(msg.chars().count() as u16) / 2, r.y + y as u16, msg, Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
                                }
                            } else {
                                let url = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
                                let (cw, ch, ox, oy) = fit(16.0 / 9.0, pw as usize, *h as usize);
                                if let Some(cells) = self.thumbs.cells(&url, cw as u16, ch as u16, &mut self.renderer) {
                                    blit(buf, r, r.x + x + ox as u16, sy + oy as i64, cells, cw, ch);
                                }
                                center_text(buf, r, r.x + x, sy, pw, *h, " ▶  Enter / click to play ");
                            }
                        }
                    }
                }
                Prim::Chat { x, y, w: cw, h, input } => {
                    let sy = *y as i64 - scroll as i64;
                    if sy + *h as i64 > 0 && sy < vh as i64 {
                        let cw = (*cw).min(w.saturating_sub(*x + 1));
                        let inp = input.then_some(self.auth.is_some());
                        draw_chat(buf, r, r.x + x, sy, cw, *h, self.chat.as_ref(), inp);
                    }
                }
                Prim::PlayerBar { x, y, w: bw, id } => {
                    if *y >= scroll && *y < scroll + vh as usize {
                        if let Some(p) = self.player.as_ref().filter(|p| &p.video.id == id) {
                            draw_bar(buf, r.x + x, r.y + (y - scroll) as u16, (*bw).min(w.saturating_sub(*x)), p, self.renderer.mode, false);
                        }
                    }
                }
            }
        }
        // Focus ring: accent bar in the gutter, inverted for one-line targets.
        if self.side_focus.is_none() && self.edit.is_none() {
            if let Some(t) = page.focus.and_then(|i| doc.targets.get(i)) {
                for yy in t.y..t.y + t.h as usize {
                    if yy >= scroll && yy < scroll + vh as usize {
                        let sy = r.y + (yy - scroll) as u16;
                        if t.x > 0 {
                            buf[(r.x + t.x - 1, sy)].set_char('▌').set_fg(ACCENT);
                        }
                        if t.h == 1 {
                            buf.set_style(Rect { x: r.x + t.x, y: sy, width: t.w.min(w.saturating_sub(t.x)), height: 1 }, Style::default().add_modifier(Modifier::REVERSED));
                        }
                    }
                }
            }
        }
        // Scrollbar
        if doc.h > vh as usize {
            let th = ((vh as usize * vh as usize) / doc.h).max(1);
            let ty = (scroll * (vh as usize - th)) / max_scroll.max(1);
            for i in 0..vh {
                let on = (i as usize) >= ty && (i as usize) < ty + th;
                buf[(r.x + w - 1, r.y + i)].set_char(if on { '┃' } else { '│' }).set_fg(if on { Color::Rgb(140, 140, 150) } else { Color::Rgb(40, 40, 48) });
            }
        }
        player_visible
    }

    fn draw_mini(&mut self, buf: &mut Buffer, pr: Rect) {
        let Some(p) = &mut self.player else { return };
        let mw = (pr.width / 3).clamp(28, 56).min(pr.width);
        let vh = ((mw as f32) * 0.28125).round() as u16;
        let h = vh + 2;
        if h + 1 > pr.height {
            return;
        }
        let r = Rect { x: pr.x + pr.width - mw - 1, y: pr.y + pr.height - h, width: mw, height: h };
        fill(buf, pr, r.x.saturating_sub(1).max(pr.x), (r.y - pr.y) as i64, mw + 1, h, Color::Rgb(20, 20, 24));
        fill(buf, r, r.x, 0, mw, vh, Color::Black);
        if self.viz {
            let t = self.t0.elapsed().as_secs_f32();
            let samples = p.samples();
            render_viz(&mut self.engine3d, &self.scenes, &mut self.analyzer, &mut self.renderer, &samples, SceneKind::Visualizer, t, &mut self.viz_last, mw as usize, vh as usize, &mut self.viz_cells);
            blit(buf, r, r.x, 0, &self.viz_cells, mw as usize, vh as usize);
        } else if let Some((cells, cw, ch, ox, oy)) = p.cells(&mut self.renderer, mw as usize, vh as usize) {
            blit(buf, r, r.x + ox as u16, oy as i64, cells, cw, ch);
        } else {
            center_text(buf, r, r.x, 0, mw, vh, "Loading…");
        }
        // Overlay controls on the video, like YouTube's miniplayer.
        let ctl = Style::default().fg(Color::White).bg(Color::Rgb(0, 0, 0)).add_modifier(Modifier::BOLD);
        let expand = Rect { x: r.x, y: r.y, width: 4, height: 1 };
        let close = Rect { x: r.x + mw - 4, y: r.y, width: 4, height: 1 };
        buf.set_string(expand.x, expand.y, " ↗  ", ctl);
        buf.set_string(close.x, close.y, "  ✕ ", ctl);
        let pp = if p.paused || p.ended { "  ▶  " } else { " ❚❚  " };
        let pr_ = Rect { x: r.x + mw / 2 - 2, y: r.y + vh / 2, width: 5, height: 1 };
        if p.paused || p.ended {
            buf.set_string(pr_.x, pr_.y, pp, ctl);
        }
        self.geo.mini_btns = vec![(expand, MiniBtn::Expand), (close, MiniBtn::Close), (pr_, MiniBtn::Pause)];
        let title = page::trunc(&p.video.title, mw as usize - 8);
        buf.set_string(r.x + 1, r.y + vh, title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        let pb = Rect { x: r.x + mw - 5, y: r.y + vh, width: 4, height: 1 };
        buf.set_string(pb.x, pb.y, if p.paused || p.ended { " ▶ " } else { " ❚❚" }, Style::default().fg(Color::White).bg(page::BUTTON));
        self.geo.mini_btns.push((pb, MiniBtn::Pause));
        draw_bar(buf, r.x, r.y + vh + 1, mw, p, self.renderer.mode, true);
        self.geo.mini = Some(r);
        self.geo.mini_bar = Some(Rect { x: r.x, y: r.y + vh + 1, width: mw.saturating_sub(16), height: 1 });
    }

    fn draw_fullscreen(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(p) = &mut self.player else { return };
        buf.set_style(area, Style::default().bg(Color::Black));
        let vh = area.height - 1;
        let side = self.store.show_chat && self.chat.as_ref().is_some_and(|c| c.video == p.video.id) && area.width >= 100;
        let vw = if side { area.width - 44 } else { area.width };
        let varea = Rect { width: vw, ..area };
        if self.viz {
            let t = self.t0.elapsed().as_secs_f32();
            let samples = p.samples();
            render_viz(&mut self.engine3d, &self.scenes, &mut self.analyzer, &mut self.renderer, &samples, SceneKind::Visualizer, t, &mut self.viz_last, vw as usize, vh as usize, &mut self.viz_cells);
            blit(buf, varea, area.x, 0, &self.viz_cells, vw as usize, vh as usize);
        } else {
            match p.cells(&mut self.renderer, vw as usize, vh as usize) {
                Some((cells, cw, ch, ox, oy)) => blit(buf, varea, area.x + ox as u16, oy as i64, cells, cw, ch),
                None => center_text(buf, varea, area.x, 0, vw, vh, "Loading video…"),
            }
        }
        if side {
            let inp = self.chat.as_ref().is_some_and(|c| !c.replay).then_some(self.auth.is_some());
            draw_chat(buf, area, area.x + vw + 1, 0, 43, vh, self.chat.as_ref(), inp);
        }
        let br = Rect { y: area.y + vh, height: 1, ..area };
        buf.set_style(br, Style::default().bg(Color::Rgb(40, 0, 0)));
        draw_bar(buf, br.x, br.y, br.width, p, self.renderer.mode, false);
        self.geo.fs_bar = Some(Rect { width: br.width.saturating_sub(18), ..br });
    }

    fn draw_status(&mut self, buf: &mut Buffer, r: Rect) {
        let style = Style::default().bg(Color::Rgb(40, 0, 0)).fg(Color::Rgb(230, 230, 230));
        buf.set_style(r, style);
        let mut left = String::new();
        if self.status.1.elapsed() < Duration::from_secs(4) && !self.status.0.is_empty() {
            left = self.status.0.clone();
        } else if self.page().loading || self.page().more_loading {
            left = "↻ loading…".into();
        } else if let Some((v, _)) = &self.pending {
            left = format!("↻ opening {}…", v.title);
        } else if let Some(p) = &self.player {
            let icon = if p.ended { "■" } else if p.paused { "❚❚" } else { "▶" };
            left = format!("{icon} {} · {}", p.video.title, fmt_time(p.pos));
            if !p.note.is_empty() {
                left.push_str(&format!(" · {}", p.note));
            }
        }
        let audio = if self.opts.audio == crate::player::Audio::None {
            "no audio".to_string()
        } else if self.player.as_ref().is_some_and(|p| p.muted()) {
            "muted".to_string()
        } else {
            format!("vol {:.0}%", self.store.volume * 100.0)
        };
        let right = format!(" {} · {audio} │ / search  ↵ open  H back  f full  +- vol  C chat  ? help ", self.renderer.mode.name());
        let rw = right.width() as u16;
        let lw = r.width.saturating_sub(rw + 1) as usize;
        buf.set_string(r.x + 1, r.y, page::trunc(&left, lw), style);
        if r.width > rw {
            buf.set_string(r.x + r.width - rw, r.y, &right, style.fg(Color::Rgb(200, 160, 160)));
        }
    }

    fn draw_suggestions(&mut self, buf: &mut Buffer) {
        let Some(e) = &self.edit else { return };
        let Some(&(addr, _)) = self.geo.btns.iter().find(|(_, b)| *b == Btn::Addr) else { return };
        for (i, sgt) in e.sugg.iter().enumerate() {
            let y = addr.y + 1 + i as u16;
            let r = Rect { x: addr.x, y, width: addr.width, height: 1 };
            let sel = e.sel == Some(i);
            let style = if sel { Style::default().bg(ACCENT).fg(Color::White) } else { Style::default().bg(Color::Rgb(30, 30, 36)).fg(Color::White) };
            for xx in r.x..r.x + r.width {
                buf[(xx, y)].set_char(' ');
            }
            buf.set_style(r, style);
            buf.set_line(r.x + 2, y, &Line::from(vec![Span::styled("⌕ ", style.fg(if sel { Color::White } else { DIM })), Span::styled(sgt.clone(), style)]), r.width - 2);
            self.geo.sugg.push((r, i));
        }
    }

    // ------------------------------------------------------------ main loop

    pub fn run(&mut self, terminal: &mut DefaultTerminal, rx: Receiver<Msg>) -> Result<()> {
        use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
        self.sync_subs(false);
        let mut last_save = Instant::now();
        while !self.quit {
            while let Ok(m) = rx.try_recv() {
                self.on_msg(m);
            }
            let fresh = self.player.as_mut().is_some_and(|p| p.tick()) || self.pending.is_some() || self.player.as_ref().is_some_and(|p| p.loading());
            self.after_tick();
            // Infinite scroll: fetch more when the viewport nears the end.
            if !self.fullscreen {
                let vh = self.geo.page.height as usize;
                let near_end = {
                    let p = self.page();
                    p.layout.as_ref().is_some_and(|l| p.scroll + vh * 2 + 4 >= l.1.h) && p.more_token().is_some()
                };
                if near_end {
                    self.load_more();
                }
            }
            if self.dirty || fresh {
                let _ = crossterm::execute!(std::io::stdout(), BeginSynchronizedUpdate);
                terminal.draw(|f| self.draw(f))?;
                let _ = crossterm::execute!(std::io::stdout(), EndSynchronizedUpdate);
                self.dirty = false;
            }
            if last_save.elapsed() > Duration::from_secs(20) {
                self.store.save();
                last_save = Instant::now();
            }
            let playing = self.player.as_ref().is_some_and(|p| p.playing());
            let timeout = Duration::from_millis(if playing { 4 } else { 40 });
            if event::poll(timeout)? {
                loop {
                    let ev = event::read()?;
                    self.on_event(ev);
                    self.dirty = true;
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        self.stop();
        self.store.save();
        Ok(())
    }
}

/// Render a 3D scene into `out` (w x h cells) through the glyph renderer,
/// with settings suited to 3D (dark crush, colour-value density, classic
/// ASCII in place of the two-tone mode).
#[allow(clippy::too_many_arguments)]
fn render_viz(
    engine: &mut Engine,
    scenes: &Scenes,
    analyzer: &mut Analyzer,
    r: &mut Renderer,
    samples: &[f32],
    kind: SceneKind,
    t: f32,
    last: &mut Instant,
    w: usize,
    h: usize,
    out: &mut Vec<Cell>,
) {
    let dt = last.elapsed().as_secs_f32().clamp(0.0, 0.2);
    *last = Instant::now();
    analyzer.update(samples, dt);
    let bands: &[f32] = if samples.is_empty() { &[] } else { &analyzer.bands };
    let (scene, cam) = scenes.build(kind, t, bands, Orbit::default());
    let px = engine.render(&scene, &cam, w * GW, h * GH);
    let saved = (r.mode, r.black, r.value_density);
    if r.mode == Mode::Ascii {
        r.mode = Mode::Classic;
    }
    r.black = 0.2;
    r.value_density = true;
    r.render(&crate::render::Img { w: w * GW, h: h * GH, px }, w, h, out);
    (r.mode, r.black, r.value_density) = saved;
}

fn blit(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, cells: &[Cell], w: usize, h: usize) {
    for yy in 0..h {
        let y = sy + yy as i64;
        if y < 0 || y >= clip.height as i64 {
            continue;
        }
        for xx in 0..w {
            let cx = x + xx as u16;
            if cx >= clip.x + clip.width {
                break;
            }
            let c = cells[yy * w + xx];
            buf[(cx, clip.y + y as u16)]
                .set_char(c.ch)
                .set_fg(Color::Rgb(c.fg[0], c.fg[1], c.fg[2]))
                .set_bg(Color::Rgb(c.bg[0], c.bg[1], c.bg[2]));
        }
    }
}

fn fill(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, w: u16, h: u16, color: Color) {
    let y0 = sy.max(0);
    let y1 = (sy + h as i64).min(clip.height as i64);
    if y1 <= y0 {
        return;
    }
    let w = w.min((clip.x + clip.width).saturating_sub(x));
    let r = Rect { x, y: clip.y + y0 as u16, width: w, height: (y1 - y0) as u16 };
    for y in r.y..r.y + r.height {
        for xx in r.x..r.x + r.width {
            buf[(xx, y)].set_char(' ').set_bg(color);
        }
    }
}

fn center_text(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, w: u16, h: u16, text: &str) {
    let y = sy + h as i64 / 2;
    if y < 0 || y >= clip.height as i64 {
        return;
    }
    let tw = text.width() as u16;
    let tx = x + w.saturating_sub(tw) / 2;
    buf.set_string(tx, clip.y + y as u16, text, Style::default().fg(Color::White).bg(Color::Rgb(0, 0, 0)).add_modifier(Modifier::BOLD));
}

fn draw_bar(buf: &mut Buffer, x: u16, y: u16, w: u16, p: &Player, mode: Mode, compact: bool) {
    let d = p.duration();
    let icon = if p.loading() { "…" } else if p.ended { "■" } else if p.paused { "❚❚" } else { "▶" };
    let time = if p.stream.live { format!("{icon} LIVE {}", fmt_time(p.pos)) } else { format!("{icon} {} / {}", fmt_time(p.pos), fmt_time(d)) };
    let vol = if p.muted() { "muted".to_string() } else { format!("vol {:.0}%", p.level() * 100.0) };
    let ae = if p.exposure.on { " AE" } else { "" };
    let tail = if compact { format!(" {time} ") } else { format!(" {time}  {vol}  {}{ae} {:.0}fps ", mode.name(), p.fps) };
    let tw = tail.width() as u16;
    let bw = w.saturating_sub(tw.max(16) + 1);
    let filled = if d > 0.0 { ((p.pos / d).clamp(0.0, 1.0) * bw as f64) as u16 } else { 0 };
    for i in 0..bw {
        let c = &mut buf[(x + i, y)];
        if i < filled {
            c.set_char('━').set_fg(Color::Rgb(255, 0, 0));
        } else if i == filled {
            c.set_char('●').set_fg(Color::Rgb(255, 40, 40));
        } else {
            c.set_char('━').set_fg(Color::Rgb(80, 80, 88));
        }
    }
    buf.set_string(x + w.saturating_sub(tw), y, &tail, Style::default().fg(Color::White));
}

fn role_color(r: Role) -> Color {
    match r {
        Role::Owner => Color::Rgb(255, 214, 0),
        Role::Moderator => Color::Rgb(94, 132, 241),
        Role::Member => Color::Rgb(43, 186, 74),
        Role::Verified => Color::Rgb(230, 230, 230),
        Role::Normal => Color::Rgb(160, 160, 170),
    }
}

/// Chat panel: header, then the newest messages that fit, bottom-aligned.
/// `input`: Some(signed_in) reserves the bottom row for the send box.
fn draw_chat(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, w: u16, h: u16, chat: Option<&Chat>, input: Option<bool>) {
    if w < 12 || h < 4 {
        return;
    }
    let h = if let Some(signed_in) = input {
        let y = sy + h as i64 - 1;
        if y >= 0 && y < clip.height as i64 {
            let r = Rect { x, y: clip.y + y as u16, width: w, height: 1 };
            fill(buf, clip, x, y, w, 1, Color::Rgb(40, 40, 50));
            let msg = if signed_in { " Say something…  (I)" } else { " Sign in to chat" };
            buf.set_string(r.x, r.y, page::trunc(msg, w as usize), Style::default().fg(DIM));
        }
        h - 1
    } else {
        h
    };
    fill(buf, clip, x, sy, w, h, Color::Rgb(24, 24, 30));
    let row = |i: u16| -> Option<u16> {
        let y = sy + i as i64;
        (y >= 0 && y < clip.height as i64).then(|| clip.y + y as u16)
    };
    let head = match chat {
        Some(c) if c.replay => vec![Span::styled(" Chat replay", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))],
        _ => vec![
            Span::styled(" ● ", Style::default().fg(ACCENT)),
            Span::styled("Live chat", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ],
    };
    if let Some(y) = row(0) {
        buf.set_line(x, y, &Line::from(head), w);
        let hint = "C hide ";
        buf.set_string(x + w - hint.len() as u16, y, hint, Style::default().fg(DIM));
    }
    if let Some(y) = row(1) {
        buf.set_string(x, y, "─".repeat(w as usize), Style::default().fg(Color::Rgb(50, 50, 60)));
    }
    let body = h.saturating_sub(2) as usize;
    let tw = (w - 2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    if let Some(c) = chat {
        for m in c.visible().rev() {
            let prefix_w = m.author.width() + 2 + m.amount.as_ref().map_or(0, |a| a.width() + 3);
            // Non-breaking spaces: the wrapper collapses ordinary space runs.
            let pad = "\u{a0}".repeat(prefix_w);
            let wrapped = page::wrap(&format!("{pad}{}", m.text), tw, 6);
            let mut ls: Vec<Line> = Vec::new();
            for (i, l) in wrapped.into_iter().enumerate() {
                if i == 0 {
                    let mut spans = Vec::new();
                    if let Some(a) = &m.amount {
                        spans.push(Span::styled(format!(" {a} "), Style::default().fg(Color::Black).bg(Color::Rgb(255, 202, 40)).add_modifier(Modifier::BOLD)));
                        spans.push(Span::raw(" "));
                    }
                    let name_style = match m.role {
                        Role::Owner => Style::default().fg(Color::Black).bg(role_color(m.role)),
                        r => Style::default().fg(role_color(r)),
                    };
                    spans.push(Span::styled(m.author.clone(), name_style.add_modifier(Modifier::BOLD)));
                    spans.push(Span::raw("  "));
                    let rest: String = l.chars().skip(pad.chars().count()).collect();
                    spans.push(Span::styled(rest, Style::default().fg(Color::Rgb(225, 225, 225))));
                    ls.push(Line::from(spans));
                } else {
                    ls.push(Line::from(Span::styled(l, Style::default().fg(Color::Rgb(225, 225, 225)))));
                }
            }
            for l in ls.into_iter().rev() {
                lines.push(l);
            }
            if lines.len() >= body {
                break;
            }
        }
    }
    lines.truncate(body);
    lines.reverse();
    if lines.is_empty() {
        let msg = match chat {
            Some(c) => c.error.clone().unwrap_or_else(|| if c.replay { "Waiting for messages…".into() } else { "Connecting to chat…".into() }),
            None => "Chat starts when the video plays".into(),
        };
        if let Some(y) = row(2) {
            buf.set_string(x + 1, y, page::trunc(&msg, tw), Style::default().fg(DIM));
        }
        return;
    }
    let top = 2 + (body - lines.len()) as u16;
    for (i, l) in lines.iter().enumerate() {
        if let Some(y) = row(top + i as u16) {
            buf.set_line(x + 1, y, l, w - 1);
        }
    }
}

// ---------------------------------------------------------------- feeds

fn rss_all(api: &Api, ids: &[String]) -> Vec<VideoItem> {
    let mut all: Vec<(u64, VideoItem)> = Vec::new();
    for chunk in ids.chunks(12) {
        let got: Vec<Vec<(u64, VideoItem)>> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk.iter().map(|id| sc.spawn(move || api.rss(id).unwrap_or_default())).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        all.extend(got.into_iter().flatten());
    }
    all.sort_by(|a, b| b.0.cmp(&a.0));
    all.into_iter().map(|x| x.1).collect()
}

/// "For you": related videos of recent watches interleaved with new
/// uploads from subscriptions, skipping anything already watched.
fn for_you(api: &Api, hist: &[String], subs: &[String], watched: &std::collections::HashSet<String>) -> Feed {
    let rel: Vec<Vec<VideoItem>> = std::thread::scope(|sc| {
        let hs: Vec<_> = hist.iter().map(|id| sc.spawn(move || api.watch(id).map(|w| w.related.videos()).unwrap_or_default())).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    let fresh: Vec<VideoItem> = rss_all(api, &subs[..subs.len().min(24)]).into_iter().filter(|v| !v.short).take(24).collect();
    let mut lists: Vec<std::vec::IntoIter<VideoItem>> = rel.into_iter().map(|v| v.into_iter()).collect();
    lists.push(fresh.into_iter());
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    loop {
        let mut any = false;
        for l in lists.iter_mut() {
            for v in l.by_ref() {
                any = true;
                if !v.live && !watched.contains(&v.id) && seen.insert(v.id.clone()) {
                    out.push(Entry::Item(Item::Video(v)));
                    break;
                }
            }
        }
        if !any || out.len() >= 80 {
            break;
        }
    }
    Feed { entries: out, cont: None }
}
