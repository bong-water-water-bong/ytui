//! The browser: tabs with back/forward history, an address bar with search
//! suggestions, a sidebar, pages laid out as scrollable documents, mouse and
//! spatial keyboard navigation, and one video player that plays inline on
//! its watch page, in a miniplayer elsewhere, or fullscreen.

use crate::api::{self, Api, ChannelPage, Comment, CommentBatch, Entry, Feed, Item, PlaylistPage, Role, Target, VideoItem, WatchInfo};
use crate::auth::{Auth, MyPlaylist, PlItem};
use crate::chat::{self, Chat};
use crate::page::{MRoute, MusicNow, self, chip_query, layout, Act, Ctx, Doc, Kind, Page, Prim, Watch, ACCENT, CHIPS, DIM, FILTERS, META};
use crate::player::{fit, Opts, Player};
use crate::render::{Cell, Renderer};
use crate::store::Store;
use crate::thumbs::Thumbs;
use crate::yt::{self, fmt_time};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::{Frame, Terminal};

/// ratatui writing into the frame buffer that `out::Out` sends.
pub type Term = Terminal<ratatui::backend::CrosstermBackend<crate::out::Buf>>;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

type R<T> = std::result::Result<T, String>;

pub enum Msg {
    Wake,
    /// A status-line message from a background thread.
    Flash(String),
    /// Name of the signed-in YouTube account (None: signed out).
    AccountName(Option<String>),
    /// Channels the signed-in account subscribes to.
    RemoteSubs(Vec<crate::api::ChannelItem>),
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
    /// A YouTube Music playlist's queue, to start playing it.
    MusicStart(String, R<crate::music::Next>),
    /// A library toggle finished: (video, message).
    Library(String, R<String>),
    /// A song's credits, for the credits dialog: (song title, credits).
    Credits(String, R<Vec<(String, String)>>),
}

#[derive(Clone, Copy, PartialEq)]
enum MiniBtn {
    Expand,
    Close,
    Pause,
}

/// A button in the controls drawn over the video.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctl {
    Prev,
    Pause,
    Next,
    Mute,
    Captions,
    Full,
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
    /// A song menu: its song, and the screen cell it pops out from.
    song: Option<Box<crate::music::Song>>,
    at: Option<(u16, u16)>,
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
    // Song menu
    Mix,
    PlayNext,
    AddQueue,
    /// Library toggle (label, feedback token; empty when signed out).
    Library(String, String),
    Like,
    Download,
    SavePl,
    Unqueue,
    Album(String),
    Artist(String),
    Credits,
    Share,
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
    Music(R<crate::music::MusicPage>),
    MusicMore(R<crate::music::MusicPage>),
    MusicNext(R<crate::music::Next>),
    MusicLyrics(R<crate::music::Lyrics>),
    MusicRelated(R<crate::music::MusicPage>),
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
    Music,
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
    /// Video boxes with the player in them, and their control buttons.
    players: Vec<Rect>,
    ctl: Vec<(Rect, Ctl)>,
    /// Player-bar volume sliders and seek bars drawn this frame.
    vol: Vec<Rect>,
    seek: Vec<Rect>,
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
    /// Bytes to write after the current frame.
    frame_out: Vec<u8>,
    /// Chat source for a video: (video id, token, is_replay).
    chat_src: Option<(String, String, bool)>,
    up_next: (String, Vec<VideoItem>),
    /// The YouTube Music queue playing (an album, playlist or radio), so
    /// the music player keeps it as songs advance or it's reopened.
    music_list: Vec<crate::music::Song>,
    queue: Vec<VideoItem>,
    qpos: usize,
    fullscreen: bool,
    /// The controls over the video show until then (and while paused).
    controls_until: Instant,
    /// Google sign-in window, and when it was last checked.
    login: Option<crate::browser_login::Login>,
    /// Signed-in YouTube account name, for the sidebar and Account page.
    account_name: Option<String>,
    /// A mouse drag that started on a player-bar control.
    drag: Option<Drag>,
    /// Waiting for a cookie extension's export on the clipboard since .0;
    /// .1 is the clipboard sequence number last looked at.
    capture: Option<(Instant, u32)>,
    /// Screen cell of the click being handled (song menus pop out there).
    click_at: Option<(u16, u16)>,
    /// --fullscreen: go fullscreen when the first video starts.
    fs_on_play: bool,
    /// YTUI_STATS: per-second frame timing log.
    stats: Option<Stats>,
    status: (String, Instant),
    opts: Opts,
    /// --height: fixed stream height (None: chosen per output).
    max_h: Option<u32>,
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
    /// Stream height to ask for: --height if given, otherwise 720, the
    /// lowest height YouTube serves at 60 fps. Terminal output is never
    /// sharper than that, and 720p decodes cheaply in hardware.
    fn stream_height(&self) -> u32 {
        self.max_h.unwrap_or(720)
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.fs_on_play = on;
    }

    /// Show a startup hint in the status bar (longer than a normal flash).
    pub fn hint(&mut self, msg: String) {
        self.status = (msg, Instant::now() + Duration::from_secs(8));
    }

    pub fn new(renderer: Renderer, opts: Opts, max_h: Option<u32>) -> (App, Receiver<Msg>) {
        let (tx, rx) = channel();
        let txw = tx.clone();
        let thumbs = Thumbs::new(move || {
            let _ = txw.send(Msg::Wake);
        });
        let store = Store::load();
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
            frame_out: Vec::new(),
            chat_src: None,
            up_next: (String::new(), Vec::new()),
            music_list: Vec::new(),
            queue: Vec::new(),
            qpos: 0,
            fullscreen: false,
            controls_until: Instant::now(),
            login: None,
            account_name: None,
            drag: None,
            capture: None,
            click_at: None,
            stats: std::env::var("YTUI_STATS").ok().map(Stats::new),
            fs_on_play: false,
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
                        // Signed in: YouTube's own recommendations for you.
                        if api.signed_in() {
                            if let Ok(f) = api.home() {
                                if !f.entries.is_empty() {
                                    return send(Load::Feed(Ok(f)));
                                }
                            }
                        }
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
                if let Some(m) = &w.music {
                    let (api, pl, id, send) = (api.clone(), m.playlist.clone(), id.clone(), send.clone());
                    std::thread::spawn(move || send(Load::MusicNext(s(api.music_next(Some(&id), pl.as_deref())))));
                }
                std::thread::spawn(move || send(Load::Watch(s(api.watch(&id)))));
            }
            Kind::Music { route, .. } => {
                let route = route.clone();
                let signed_in = api.signed_in();
                std::thread::spawn(move || {
                    let r = match route {
                        MRoute::Home => api.music_browse(&crate::music::Browse::new("FEmusic_home")),
                        MRoute::Explore => api.music_browse(&crate::music::Browse::new("FEmusic_explore")),
                        MRoute::Library(_) if !signed_in => return send(Load::Music(Err("sign in to see your library".into()))),
                        MRoute::Library(i) => api.music_browse(&crate::music::Browse::new(crate::music::LIBRARY[i].1)),
                        MRoute::Search { q, filter: 0 } => api.music_search_all(&q),
                        MRoute::Search { q, filter } => api.music_search(&q, crate::music::FILTERS[filter].1),
                        MRoute::Browse(b) => api.music_browse(&b),
                    };
                    send(Load::Music(s(r)));
                });
            }
            Kind::Subs { .. } => {
                std::thread::spawn(move || {
                    // Signed in: your YouTube subscriptions feed.
                    if api.signed_in() {
                        if let Ok(f) = api.subscriptions_feed() {
                            if !f.entries.is_empty() {
                                return send(Load::Feed(Ok(f)));
                            }
                        }
                    }
                    let entries = rss_all(&api, &subs).into_iter().take(90).map(|v| Entry::Item(Item::Video(v))).collect();
                    send(Load::Feed(Ok(Feed { entries, cont: None })));
                });
            }
            Kind::Playlists { .. } => match self_auth.clone() {
                Some(a) => {
                    std::thread::spawn(move || send(Load::Playlists(s(a.playlists()))));
                }
                None if api.signed_in() => {
                    std::thread::spawn(move || {
                        let r = api.my_playlists().map(|l| {
                            l.into_iter()
                                .map(|p| crate::auth::MyPlaylist {
                                    count: p.count.as_deref().map(|c| c.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>()).and_then(|d| d.parse().ok()).unwrap_or(0),
                                    id: p.id,
                                    title: p.title,
                                    privacy: String::new(),
                                    thumb: p.thumb,
                                })
                                .collect()
                        });
                        send(Load::Playlists(s(r)));
                    });
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
            Kind::Music { route, .. } => {
                let search = matches!(route, MRoute::Search { .. });
                std::thread::spawn(move || send(Load::MusicMore(s(api.music_more(&tok, search)))))
            }
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
        let mut music_queue: Option<(String, Vec<VideoItem>, usize)> = None;
        let mut music_songs: Option<Vec<crate::music::Song>> = None;
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
            (Load::Music(Ok(mut m)), Kind::Music { page, route }) => {
                // An album's tracks carry no art or album of their own.
                if let (Some(h), MRoute::Browse(b)) = (&m.header, &*route) {
                    let album = b.id.starts_with("MPRE").then(|| (h.title.clone(), b.id.clone()));
                    let art = h.thumb.clone().filter(|_| !h.banner);
                    for sec in &mut m.sections {
                        for it in &mut sec.items {
                            if let crate::music::MItem::Song(s) = it {
                                if s.thumb.is_none() {
                                    s.thumb = art.clone();
                                }
                                if s.album.is_none() {
                                    s.album = album.clone();
                                }
                                if s.video.channel.is_empty() && album.is_some() {
                                    if let Some((name, id)) = &h.artist {
                                        s.video.channel = name.clone();
                                        s.video.channel_id = Some(id.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                *page = Some(m);
            }
            (Load::Music(Err(e)), _) => p.error = Some(e),
            (Load::MusicMore(r), Kind::Music { page, .. }) => {
                p.more_loading = false;
                match (r, page) {
                    (Ok(m), Some(pg)) => pg.extend(m),
                    (Err(e), _) => err_flash = Some(e),
                    _ => {}
                }
            }
            (Load::MusicNext(r), Kind::Watch(w)) => {
                if let Some(m) = &mut w.music {
                    match r {
                        Ok(mut n) => {
                            // Played from a list: that list (with its own
                            // titles) stays the queue; else the radio is.
                            if m.list.len() > 1 {
                                n.queue = m.list.clone();
                            }
                            let at = n.queue.iter().position(|q| q.video.id == w.video.id);
                            if at.is_some() {
                                music_songs = Some(n.queue.clone());
                            }
                            if m.album.is_none() {
                                m.album = at.and_then(|i| n.queue[i].album.clone());
                            }
                            // The music queue becomes what plays next.
                            if let Some(i) = at {
                                music_queue = Some((w.video.id.clone(), n.queue.iter().map(|q| q.video.clone()).collect::<Vec<_>>(), i));
                                w.queue = n.queue.iter().map(|q| q.video.clone()).collect();
                                w.qpos = i;
                            }
                            m.next = Some(n);
                        }
                        Err(e) => {
                            m.next = Some(Default::default());
                            err_flash = Some(e);
                        }
                    }
                }
            }
            (Load::MusicLyrics(r), Kind::Watch(w)) => {
                if let Some(m) = &mut w.music {
                    m.lyrics = Some(r);
                }
            }
            (Load::MusicRelated(r), Kind::Watch(w)) => {
                if let Some(m) = &mut w.music {
                    m.related = Some(r);
                }
            }
            (Load::Playlists(Ok(l)), Kind::Playlists { list }) => *list = Some(l),
            (Load::MyPlaylist(Ok((i, it))), Kind::MyPlaylist { info, items, .. }) => {
                *info = Some(i);
                *items = it;
            }
            (Load::Playlists(Err(e)) | Load::MyPlaylist(Err(e)), _) => p.error = Some(e),
            (Load::Playlist(Ok(pl)), Kind::Playlist { page, .. }) => *page = Some(pl),
            (Load::Watch(Ok(info)), Kind::Watch(w)) => {
                if let Some(t) = info.comments_token.as_ref().filter(|_| w.music.is_none()) {
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
        if let Some((id, q, i)) = music_queue {
            if self.player.as_ref().map(|p| &p.video.id).or(self.pending.as_ref().map(|p| &p.0.id)) == Some(&id) {
                self.queue = q;
                self.qpos = i;
                if let Some(songs) = music_songs {
                    self.music_list = songs;
                }
            }
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
            Msg::Flash(m) => self.flash(m),
            Msg::AccountName(n) => {
                self.account_name = n;
                self.store_rev += 1;
            }
            Msg::RemoteSubs(chs) => {
                let before = self.store.subs.len();
                for c in chs {
                    if !self.store.subs.iter().any(|s| s.id == c.id) {
                        self.store.subs.push(crate::store::ChannelRef { id: c.id, name: c.name, sub_id: None });
                    }
                }
                if self.store.subs.len() != before {
                    self.store_changed();
                    self.refresh_signed_in_pages();
                }
            }
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
                        let mut p = Player::new(v, st, start, self.opts, (c as usize, r.saturating_sub(1) as usize), self.store.volume, self.store.exposure);
                        p.set_caption_pref(self.store.captions.clone());
                        self.player = Some(p);
                        if std::mem::take(&mut self.fs_on_play) {
                            self.fullscreen = true;
                        }
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
            Msg::MusicStart(pl, r) => match r {
                Ok(n) if !n.queue.is_empty() => {
                    let mut q = n.queue;
                    for x in &mut q {
                        x.playlist = Some(pl.clone());
                    }
                    self.open_song(&q, 0);
                }
                Ok(_) => self.flash("this playlist is empty"),
                Err(e) => self.flash(format!("⚠ {e}")),
            },
            Msg::Library(id, r) => match r {
                Ok(m) => {
                    self.library_flipped(&id);
                    self.flash(m);
                }
                Err(e) => self.flash(format!("⚠ {e}")),
            },
            Msg::Credits(title, r) => match r {
                Ok(c) => {
                    let rows = c.into_iter().map(|(role, who)| DRow { label: format!("{role}: {who}"), act: DAct::Close }).collect();
                    self.dialog = Some(Dialog { title: format!("Song credits · {}", page::trunc(&title, 40)), rows, sel: 0, video: None, member: Default::default(), loading: false, song: None, at: None });
                }
                Err(e) => self.flash(format!("⚠ {e}")),
            },
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
            music: v.music.then(|| self.music_now(&v.id)),
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
        let (api, tx, h) = (self.api.clone(), self.tx.clone(), self.stream_height());
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

    /// Previous: back to the start, or (near the start) the previous video
    /// in the queue.
    fn previous(&mut self) {
        let Some(p) = &mut self.player else { return };
        let cur = p.video.id.clone();
        let in_queue = self.queue.get(self.qpos).is_some_and(|v| v.id == cur);
        if p.pos > 5.0 || !in_queue || self.qpos == 0 {
            p.seek_to(0.0);
            return;
        }
        let (queue, qpos) = (self.queue.clone(), self.qpos - 1);
        let v = queue[qpos].clone();
        if matches!(&self.page().kind, Kind::Watch(w) if w.video.id == cur) {
            self.open_video(v, queue, qpos);
        } else {
            self.play(v, queue, qpos, None);
        }
    }

    fn toggle_mute(&mut self) {
        if let Some(p) = &mut self.player {
            p.toggle_mute();
            let msg = if p.muted() { "muted".to_string() } else { format!("volume {:.0}%", p.level() * 100.0) };
            self.flash(msg);
        }
    }

    fn cycle_captions(&mut self) {
        if let Some(p) = &mut self.player {
            let (msg, pref) = p.cycle_captions();
            if pref != self.store.captions {
                self.store.captions = pref;
                self.store_changed();
            }
            self.flash(msg);
        }
    }

    /// Keep the controls over the video up for a few more seconds.
    fn show_controls(&mut self) {
        self.controls_until = Instant::now() + Duration::from_secs(3);
    }

    /// Controls show while paused, ended or loading, and for a while after
    /// the mouse moves over the video or a player key is pressed.
    fn controls_on(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.paused || p.ended || p.loading()) || Instant::now() < self.controls_until
    }

    fn control(&mut self, c: Ctl) {
        self.show_controls();
        match c {
            Ctl::Prev => self.previous(),
            Ctl::Pause => {
                if let Some(p) = &mut self.player {
                    p.toggle_pause();
                }
            }
            Ctl::Next => self.advance(),
            Ctl::Mute => self.toggle_mute(),
            Ctl::Captions => self.cycle_captions(),
            Ctl::Full => self.fullscreen = !self.fullscreen && self.player.is_some(),
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

    /// Look up the signed-in account: its name and subscribed channels.
    fn fetch_account(&self) {
        let (tx, api) = (self.tx.clone(), self.api.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::AccountName(api.account_name().ok()));
            if let Ok(chs) = api.subscribed_channels() {
                let _ = tx.send(Msg::RemoteSubs(chs));
            }
        });
    }

    /// Reload Home and Subscriptions after signing in or out, so they show
    /// (or stop showing) the account's own feeds.
    fn refresh_signed_in_pages(&mut self) {
        for t in 0..self.tabs.len() {
            for i in 0..self.tabs[t].pages.len() {
                let p = &mut self.tabs[t].pages[i];
                match &mut p.kind {
                    Kind::Home { feed, .. } | Kind::Subs { feed } => *feed = Feed::default(),
                    Kind::Playlists { list } => *list = None,
                    _ => continue,
                }
                p.rev += 1;
                let uid = p.uid;
                self.load(uid);
            }
        }
    }

    /// While a sign-in window is open, check on it.
    fn poll_login(&mut self) {
        use crate::browser_login::Poll;
        let Some(l) = self.login.as_mut() else { return };
        let r = l.poll();
        let timed_out = matches!(r, Poll::Waiting) && l.started.elapsed() > Duration::from_secs(900);
        match r {
            Poll::Waiting if !timed_out => return,
            Poll::Waiting => {
                if let Some(l) = self.login.take() {
                    l.close();
                }
                self.flash("gave up waiting for the sign-in");
            }
            Poll::Done(c) => {
                self.login = None;
                self.take_cookies(c);
            }
            Poll::NotSignedIn => {
                self.login = None;
                self.flash("the sign-in window closed before you were signed in to YouTube");
            }
            Poll::Failed(e) => {
                self.login = None;
                self.flash(format!("sign-in failed: {e}; try \"use my cookie extension\""));
            }
        }
        self.store_rev += 1;
    }

    /// While waiting on a cookie extension, look at the clipboard whenever
    /// it changes.
    fn poll_capture(&mut self) {
        let Some((since, seq)) = self.capture else { return };
        if since.elapsed() > Duration::from_secs(300) {
            self.capture = None;
            self.store_rev += 1;
            self.flash("gave up waiting for cookies; try Sign in again");
            return;
        }
        let now = crate::cookies::clipboard_seq();
        if now == seq || now == 0 {
            return;
        }
        self.capture = Some((since, now));
        if let Some(c) = crate::cookies::clipboard_text().and_then(|t| crate::cookies::parse(&t)) {
            self.take_cookies(c);
        }
    }

    /// A sign-in arrived (window, clipboard or paste): save it, clear the
    /// clipboard, and check it with YouTube.
    fn take_cookies(&mut self, c: Vec<crate::cookies::Cookie>) {
        if !crate::cookies::signed_in(&c) {
            self.flash("those cookies aren't from a signed-in youtube.com tab: sign in there and try again");
            return;
        }
        match crate::cookies::save(&c) {
            Ok(_) => {
                if self.capture.take().is_some() {
                    crate::cookies::clear_clipboard();
                }
                self.store_rev += 1;
                self.flash("signed in, checking with YouTube…");
                self.api.reload_auth();
                self.refresh_signed_in_pages();
                self.fetch_account();
                let (tx, api) = (self.tx.clone(), self.api.clone());
                std::thread::spawn(move || {
                    let name = api.account_name().ok();
                    let m = match (crate::cookies::verify(), &name) {
                        (Ok(()), Some(n)) => format!("signed in as {n} ✓"),
                        (Ok(()), None) => "signed in ✓ videos will play".to_string(),
                        (Err(e), _) => format!("YouTube didn't accept the sign-in: {e}"),
                    };
                    let _ = tx.send(Msg::AccountName(name));
                    let _ = tx.send(Msg::Flash(m));
                });
            }
            Err(e) => self.flash(format!("couldn't save the sign-in: {e}")),
        }
    }

    fn after_tick(&mut self) {
        self.poll_login();
        self.poll_capture();
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
            // YouTube Music always plays on through its queue; autoplay
            // decides for videos.
            let in_queue = v.music && self.music_list.len() > 1 && self.music_list.iter().any(|x| x.video.id == v.id);
            if self.store.autoplay || in_queue {
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

    /// Music player state for a song: within the queue playing, it keeps
    /// that queue (and the song's playlist and album).
    fn music_now(&self, id: &str) -> MusicNow {
        match self.music_list.iter().find(|s| s.video.id == id) {
            Some(s) if self.music_list.len() > 1 => MusicNow { playlist: s.playlist.clone(), album: s.album.clone(), list: self.music_list.clone(), ..Default::default() },
            _ => MusicNow::default(),
        }
    }

    /// Play `songs[i]` in the music player, the list as its queue.
    fn open_song(&mut self, songs: &[crate::music::Song], i: usize) {
        let Some(s) = songs.get(i) else { return };
        let queue: Vec<VideoItem> = songs.iter().map(|x| x.video.clone()).collect();
        let mut v = s.video.clone();
        v.music = true;
        let w = Watch {
            video: v.clone(),
            info: None,
            desc_open: false,
            comments: Vec::new(),
            comments_cont: None,
            comment_count: None,
            comments_loading: false,
            queue: queue.clone(),
            qpos: i,
            rating: None,
            music: Some(MusicNow { playlist: s.playlist.clone(), album: s.album.clone(), list: if songs.len() > 1 && s.playlist.as_deref().is_some_and(|p| !p.starts_with("RD")) { songs.to_vec() } else { Vec::new() }, ..Default::default() }),
        };
        self.navigate(Kind::Watch(Box::new(w)));
        self.play(v, if songs.len() > 1 { queue } else { Vec::new() }, i, None);
    }

    /// Open a YouTube Music item: play a song in its list, open a card's
    /// page, or play a card's video.
    fn music_item(&mut self, page: &crate::music::MusicPage, sec: usize, i: usize) {
        use crate::music::{CardKind, MItem};
        let Some(item) = page.sections.get(sec).and_then(|s| s.items.get(i)) else { return };
        match item.clone() {
            MItem::Song(_) => {
                // Album and playlist tracks play through the whole list;
                // elsewhere the song's own shelf is its queue.
                let tracks = page.header.as_ref().is_some_and(|h| !h.banner && h.thumb.is_some());
                let list: Vec<crate::music::Song> = if tracks { page.songs() } else { page.sections[sec].items.iter().filter_map(|x| if let MItem::Song(s) = x { Some(s.clone()) } else { None }).collect() };
                let MItem::Song(song) = item else { return };
                let at = list.iter().position(|x| x.video.id == song.video.id).unwrap_or(0);
                // A lone song gets a radio as its queue (from the next endpoint).
                if list.len() <= 1 {
                    self.open_song(&[song.clone()], 0);
                } else {
                    self.open_song(&list, at);
                }
            }
            MItem::Card(c) => match c.kind {
                CardKind::Open(b) => self.navigate(Kind::Music { route: MRoute::Browse(b), page: None }),
                CardKind::Play { video, playlist } => {
                    let song = crate::music::Song { video, playlist, thumb: c.thumb.clone(), ..Default::default() };
                    self.open_song(&[song], 0);
                }
            },
            MItem::Nav { to, .. } => self.navigate(Kind::Music { route: MRoute::Browse(to), page: None }),
        }
    }

    fn activate(&mut self, act: Act, click_x: Option<u16>, tgt_x: u16, tgt_w: u16) {
        match act {
            Act::MusicGo(r) => self.navigate(Kind::Music { route: r, page: None }),
            Act::MusicItem(sec, i) => {
                if let Kind::Music { page: Some(p), .. } = &self.page().kind {
                    let p = p.clone();
                    self.music_item(&p, sec, i);
                }
            }
            Act::MusicRelated(sec, i) => {
                if let Kind::Watch(w) = &self.page().kind {
                    if let Some(Ok(p)) = w.music.as_ref().and_then(|m| m.related.as_ref()) {
                        let p = p.clone();
                        self.music_item(&p, sec, i);
                    }
                }
            }
            Act::MusicQueue(i) => {
                if let Kind::Watch(w) = &self.page().kind {
                    if let Some(n) = w.music.as_ref().and_then(|m| m.next.as_ref()) {
                        let q = n.queue.clone();
                        self.open_song(&q, i);
                    }
                }
            }
            Act::SongMenu(song) => {
                let at = self.click_at.or_else(|| self.focus_cell()).unwrap_or((self.term.0 / 2, self.term.1 / 3));
                self.open_song_menu(*song, at);
            }
            Act::MusicPlaylist(pl) => {
                let (api, tx) = (self.api.clone(), self.tx.clone());
                self.flash("loading…");
                std::thread::spawn(move || {
                    let r = s(api.music_next(None, Some(&pl)));
                    let _ = tx.send(Msg::MusicStart(pl, r));
                });
            }
            Act::MusicFilter(i) | Act::MusicLib(i) => {
                let p = self.page_mut();
                if let Kind::Music { route, page } = &mut p.kind {
                    match route {
                        MRoute::Search { filter, .. } => *filter = i,
                        MRoute::Library(l) => *l = i,
                        _ => return,
                    }
                    *page = None;
                }
                p.scroll = 0;
                let uid = p.uid;
                self.load(uid);
            }
            Act::MusicTab(t) => {
                let (api, tx) = (self.api.clone(), self.tx.clone());
                let p = self.page_mut();
                let uid = p.uid;
                if let Kind::Watch(w) = &mut p.kind {
                    if let Some(m) = &mut w.music {
                        m.tab = t;
                        let next = m.next.clone().unwrap_or_default();
                        if t == 1 && m.lyrics.is_none() {
                            match next.lyrics {
                                Some(id) => {
                                    std::thread::spawn(move || {
                                        let _ = tx.send(Msg::Load(uid, Load::MusicLyrics(s(api.music_lyrics(&id)))));
                                    });
                                }
                                None if m.next.is_some() => m.lyrics = Some(Err("Lyrics aren't available for this song".into())),
                                None => {}
                            }
                        } else if t == 2 && m.related.is_none() {
                            match next.related {
                                Some(id) => {
                                    std::thread::spawn(move || {
                                        let _ = tx.send(Msg::Load(uid, Load::MusicRelated(s(api.music_browse(&crate::music::Browse::new(&id))))));
                                    });
                                }
                                None if m.next.is_some() => m.related = Some(Err("Nothing related for this song".into())),
                                None => {}
                            }
                        }
                    }
                }
                self.page_mut().changed();
            }
            Act::MusicSearch => self.start_edit(false),
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
                        song: None,
                        at: None,
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
            Act::SignIn => {
                if let Some(l) = self.login.take() {
                    l.close();
                    self.flash("sign-in cancelled");
                } else {
                    match crate::browser_login::Login::start() {
                        Ok(l) => {
                            self.login = Some(l);
                            self.flash("sign in with Google in the browser that opened");
                        }
                        Err(e) => {
                            // No usable browser: fall back to a cookie extension.
                            self.capture = Some((Instant::now(), crate::cookies::clipboard_seq()));
                            self.flash(format!("{e}; use your cookie extension instead"));
                        }
                    }
                }
                self.store_rev += 1;
            }
            Act::SignInCookies => {
                if self.capture.take().is_some() {
                    self.flash("sign-in cancelled");
                } else {
                    self.capture = Some((Instant::now(), crate::cookies::clipboard_seq()));
                    crate::sys::open_url("https://www.youtube.com");
                    self.flash("on youtube.com, click your cookie extension and press Export");
                }
                self.store_rev += 1;
            }
            Act::SignOutVideos => {
                crate::cookies::remove();
                self.api.reload_auth();
                self.account_name = None;
                self.refresh_signed_in_pages();
                self.store_rev += 1;
                self.flash("signed out");
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
        self.dialog = Some(Dialog { title, rows, sel: 0, video: Some(v.clone()), member: Default::default(), loading: signed_in, song: None, at: None });
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
        if let Some(song) = d.song.clone() {
            self.dialog = None;
            return self.song_action(*song, row.act);
        }
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
            _ => {}
        }
    }

    /// The song an item stands for, if it's a YouTube Music song.
    fn song_of(&self, act: &Act) -> Option<crate::music::Song> {
        use crate::music::{CardKind, MItem, MusicPage, Song};
        let item = |p: &MusicPage, sec: usize, i: usize| match p.sections.get(sec)?.items.get(i)? {
            MItem::Song(s) => Some(s.clone()),
            MItem::Card(c) => match &c.kind {
                CardKind::Play { video, playlist } => Some(Song { video: video.clone(), playlist: playlist.clone(), thumb: c.thumb.clone(), ..Default::default() }),
                CardKind::Open(_) => None,
            },
            MItem::Nav { .. } => None,
        };
        let music = match &self.page().kind {
            Kind::Watch(w) => w.music.as_ref(),
            _ => None,
        };
        match act {
            Act::SongMenu(s) => Some((**s).clone()),
            Act::MusicItem(sec, i) => match &self.page().kind {
                Kind::Music { page: Some(p), .. } => item(p, *sec, *i),
                _ => None,
            },
            Act::MusicRelated(sec, i) => music.and_then(|m| m.related.as_ref()).and_then(|r| r.as_ref().ok()).and_then(|p| item(p, *sec, *i)),
            Act::MusicQueue(i) => music.and_then(|m| m.next.as_ref()).and_then(|n| n.queue.get(*i).cloned()),
            Act::Video(v) if v.music => Some(Song { video: v.clone(), ..Default::default() }),
            _ => None,
        }
    }

    /// Screen cell of the focused page item.
    fn focus_cell(&self) -> Option<(u16, u16)> {
        let p = self.page();
        let t = p.focus.and_then(|i| self.doc()?.targets.get(i).cloned())?;
        let pr = self.geo.page;
        let y = (t.y as i64 - p.scroll as i64).clamp(0, pr.height as i64 - 1) as u16;
        Some((pr.x + t.x + t.w.saturating_sub(1), pr.y + y))
    }

    /// Pop out the menu for a song, YouTube Music's ⋮ menu.
    fn open_song_menu(&mut self, mut song: crate::music::Song, at: (u16, u16)) {
        song.video.music = true;
        let id = song.video.id.clone();
        let playing = self.now_id();
        let mut rows = vec![
            DRow { label: "◉  Start mix".into(), act: DAct::Mix },
            DRow { label: "↳  Play next".into(), act: DAct::PlayNext },
            DRow { label: "≡  Add to queue".into(), act: DAct::AddQueue },
        ];
        // Signed in, only rows that came with a library toggle get one;
        // signed out, it leads to signing in.
        match song.library.first().cloned() {
            Some((label, token)) => {
                let icon = if label.starts_with("Remove") { "⊖" } else { "⊕" };
                rows.push(DRow { label: format!("{icon}  {label}"), act: DAct::Library(label, token) });
            }
            None if !self.api.signed_in() => rows.push(DRow { label: "⊕  Save to library".into(), act: DAct::Library("Save to library".into(), String::new()) }),
            None => {}
        }
        rows.push(DRow { label: "♡  Add to liked songs".into(), act: DAct::Like });
        rows.push(DRow { label: "↓  Download".into(), act: DAct::Download });
        rows.push(DRow { label: "≡+ Save to playlist".into(), act: DAct::SavePl });
        if playing.as_deref() != Some(id.as_str()) && self.music_list.iter().any(|s| s.video.id == id) {
            rows.push(DRow { label: "⊖  Remove from queue".into(), act: DAct::Unqueue });
        }
        if let Some((_, album)) = &song.album {
            rows.push(DRow { label: "◎  Go to album".into(), act: DAct::Album(album.clone()) });
        }
        if let Some(a) = song.video.channel_id.clone().filter(|a| !a.is_empty()) {
            rows.push(DRow { label: "♪  Go to artist".into(), act: DAct::Artist(a) });
        }
        rows.push(DRow { label: "ⓘ  View song credits".into(), act: DAct::Credits });
        rows.push(DRow { label: "↗  Share".into(), act: DAct::Share });
        self.dialog = Some(Dialog { title: String::new(), rows, sel: 0, video: None, member: Default::default(), loading: false, song: Some(Box::new(song)), at: Some(at) });
    }

    fn song_action(&mut self, song: crate::music::Song, act: DAct) {
        let title = page::trunc(&song.video.title, 40);
        let id = song.video.id.clone();
        match act {
            DAct::Mix => {
                let s = crate::music::Song { playlist: Some(format!("RDAMVM{id}")), ..song };
                self.open_song(&[s], 0);
            }
            DAct::PlayNext => self.queue_song(song, true),
            DAct::AddQueue => self.queue_song(song, false),
            DAct::Unqueue => {
                let Some(cur) = self.now_id() else { return };
                let list: Vec<_> = self.music_list.iter().filter(|s| s.video.id != id).cloned().collect();
                self.set_music_queue(list, &cur);
                self.flash(format!("removed “{title}” from the queue"));
            }
            DAct::Library(label, token) => {
                if token.is_empty() {
                    return self.need_sign_in();
                }
                let (api, tx) = (self.api.clone(), self.tx.clone());
                let done = if label.starts_with("Remove") { format!("removed “{title}” from your library") } else { format!("saved “{title}” to your library") };
                std::thread::spawn(move || {
                    let r = api.music_feedback(&token).map(|_| done);
                    let _ = tx.send(Msg::Library(id, s(r)));
                });
            }
            DAct::Like => {
                let (api, tx, auth) = (self.api.clone(), self.tx.clone(), self.auth.clone());
                if !api.signed_in() && auth.is_none() {
                    return self.need_sign_in();
                }
                std::thread::spawn(move || {
                    let r = if api.signed_in() { api.music_like(&id) } else { auth.map(|a| a.rate(&id, "like")).unwrap_or(Ok(())) };
                    let _ = tx.send(Msg::Flash(match r {
                        Ok(()) => format!("added “{title}” to your liked songs"),
                        Err(e) => format!("⚠ {e}"),
                    }));
                });
            }
            DAct::Download => {
                let dir = crate::sys::home().join("Music").join("ytui");
                self.flash(format!("downloading “{title}” to {}…", dir.display()));
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let _ = std::fs::create_dir_all(&dir);
                    let out = std::process::Command::new(yt::ytdlp())
                        .args(["-f", "bestaudio", "--no-playlist", "--no-progress", "-P"])
                        .arg(&dir)
                        .args(["-o", "%(artist,uploader)s - %(track,title)s.%(ext)s", &format!("https://music.youtube.com/watch?v={id}")])
                        .output();
                    let _ = tx.send(Msg::Flash(match out {
                        Ok(o) if o.status.success() => format!("downloaded “{title}” to {}", dir.display()),
                        Ok(o) => format!("⚠ download failed: {}", String::from_utf8_lossy(&o.stderr).lines().rfind(|l| !l.trim().is_empty()).unwrap_or("yt-dlp failed")),
                        Err(e) => format!("⚠ couldn't run yt-dlp: {e}"),
                    }));
                });
            }
            DAct::SavePl => self.open_save_dialog(song.video),
            DAct::Album(b) | DAct::Artist(b) => self.navigate(Kind::Music { route: MRoute::Browse(crate::music::Browse::new(&b)), page: None }),
            DAct::Credits => {
                let (api, tx) = (self.api.clone(), self.tx.clone());
                let t = song.video.title.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Credits(t, s(api.music_credits(&id))));
                });
            }
            DAct::Share => {
                // OSC 52: the terminal puts the link on the clipboard.
                use base64::Engine;
                let url = format!("https://music.youtube.com/watch?v={id}");
                let b64 = base64::engine::general_purpose::STANDARD.encode(&url);
                self.frame_out.extend_from_slice(format!("\x1b]52;c;{b64}\x07").as_bytes());
                self.flash(format!("copied {url}"));
            }
            _ => {}
        }
    }

    /// A song's library toggle went through: every copy of the song now
    /// offers the other way.
    fn library_flipped(&mut self, id: &str) {
        use crate::music::{MItem, MusicPage, Song};
        fn flip(s: &mut Song, id: &str) -> bool {
            let hit = s.video.id == id && s.library.len() == 2;
            if hit {
                s.library.reverse();
            }
            hit
        }
        fn flip_page(p: &mut MusicPage, id: &str) -> bool {
            let mut hit = false;
            for it in p.sections.iter_mut().flat_map(|s| s.items.iter_mut()) {
                if let MItem::Song(s) = it {
                    hit |= flip(s, id);
                }
            }
            hit
        }
        for s in self.music_list.iter_mut() {
            flip(s, id);
        }
        for p in self.tabs.iter_mut().flat_map(|t| t.pages.iter_mut()) {
            let hit = match &mut p.kind {
                Kind::Music { page: Some(mp), .. } => flip_page(mp, id),
                Kind::Watch(w) => match &mut w.music {
                    Some(m) => {
                        let mut hit = false;
                        for s in m.list.iter_mut() {
                            hit |= flip(s, id);
                        }
                        if let Some(n) = &mut m.next {
                            for s in n.queue.iter_mut() {
                                hit |= flip(s, id);
                            }
                        }
                        if let Some(Ok(r)) = &mut m.related {
                            hit |= flip_page(r, id);
                        }
                        hit
                    }
                    None => false,
                },
                _ => false,
            };
            if hit {
                p.changed();
            }
        }
    }

    /// The video playing (or about to).
    fn now_id(&self) -> Option<String> {
        self.player.as_ref().map(|p| p.video.id.clone()).or_else(|| self.pending.as_ref().map(|p| p.0.id.clone()))
    }

    /// Play next / Add to queue. Nothing playing: it just plays.
    fn queue_song(&mut self, song: crate::music::Song, next: bool) {
        let title = page::trunc(&song.video.title, 40);
        let Some(cur) = self.now_id() else {
            return self.open_song(&[song], 0);
        };
        if song.video.id == cur {
            return self.flash("that's playing now");
        }
        let mut list = self.music_list.clone();
        if !list.iter().any(|s| s.video.id == cur) {
            // Something outside a music queue is playing: the queue starts there.
            let v = self.player.as_ref().map(|p| p.video.clone()).or_else(|| self.pending.as_ref().map(|p| p.0.clone())).unwrap_or_default();
            list = vec![crate::music::Song { video: v, ..Default::default() }];
        }
        list.retain(|s| s.video.id != song.video.id);
        let at = list.iter().position(|s| s.video.id == cur).unwrap_or(0);
        if next {
            list.insert(at + 1, song);
        } else {
            list.push(song);
        }
        self.set_music_queue(list, &cur);
        self.flash(if next { format!("“{title}” plays next") } else { format!("added “{title}” to the queue") });
    }

    /// Make `list` the music queue, `cur` playing in it, on the player and
    /// on every music player page showing `cur`.
    fn set_music_queue(&mut self, list: Vec<crate::music::Song>, cur: &str) {
        let vids: Vec<VideoItem> = list.iter().map(|s| s.video.clone()).collect();
        let qpos = list.iter().position(|s| s.video.id == cur).unwrap_or(0);
        self.queue = vids.clone();
        self.qpos = qpos;
        self.music_list = list.clone();
        for p in self.tabs.iter_mut().flat_map(|t| t.pages.iter_mut()) {
            let Kind::Watch(w) = &mut p.kind else { continue };
            if w.video.id != cur {
                continue;
            }
            let Some(m) = &mut w.music else { continue };
            m.list = list.clone();
            if let Some(n) = &mut m.next {
                n.queue = list.clone();
            }
            w.queue = vids.clone();
            w.qpos = qpos;
            p.changed();
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
        let music_page = matches!(&self.page().kind, Kind::Music { .. }) || matches!(&self.page().kind, Kind::Watch(w) if w.music.is_some());
        if let Some(r) = crate::music::route(&q) {
            if new_tab {
                self.open_tab(Kind::Home { chip: self.default_chip(), feed: Feed::default() });
            }
            let route = match r {
                crate::music::Route::Watch(v, list) => {
                    let song = crate::music::Song { video: VideoItem { id: v, music: true, ..Default::default() }, playlist: list, ..Default::default() };
                    return self.open_song(&[song], 0);
                }
                crate::music::Route::Home => MRoute::Home,
                crate::music::Route::Explore => MRoute::Explore,
                crate::music::Route::Library => MRoute::Library(0),
                crate::music::Route::Search(q) => MRoute::Search { q, filter: 0 },
                crate::music::Route::Browse(b) => MRoute::Browse(b),
            };
            return self.navigate(Kind::Music { route, page: None });
        }
        if music_page && !api::looks_like_url(&q) {
            let kind = Kind::Music { route: MRoute::Search { q, filter: 0 }, page: None };
            return if new_tab { self.open_tab(kind) } else { self.navigate(kind) };
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
            Kind::Music { page, .. } => *page = None,
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
            ("♫  Music".to_string(), Side::Music),
            ("↺  History".to_string(), Side::History),
            ("◷  Watch later".to_string(), Side::Later),
            ("≡  Playlists".to_string(), Side::Playlists),
            ("?  Help".to_string(), Side::Help),
            (
                match (&self.account_name, self.auth.is_some()) {
                    (Some(n), _) => format!("@  {}", page::trunc(n.split(" (").next().unwrap_or(n), 18)),
                    (None, true) => "@  Account ✓".to_string(),
                    (None, false) => "@  Sign in".to_string(),
                },
                Side::Account,
            ),
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
            Side::Music => Kind::Music { route: MRoute::Home, page: None },
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
                // Cookies pasted anywhere (that isn't a text box): sign in.
                if self.compose.is_none() && self.edit.is_none() {
                    if let Some(c) = crate::cookies::parse(&t) {
                        self.take_cookies(c);
                        return;
                    }
                }
                if let Some(c) = &mut self.compose {
                    c.text.push_str(&t);
                } else if let Some(e) = &mut self.edit {
                    e.text.push_str(t.lines().next().unwrap_or(""));
                    self.suggest();
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Moved => {
                    if self.geo.players.iter().any(|r| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height) {
                        self.show_controls();
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.click_at = Some((m.column, m.row));
                    self.click(m.column, m.row);
                    self.click_at = None;
                }
                MouseEventKind::Down(MouseButton::Right) => self.right_click(m.column, m.row),
                MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                    Some(Drag::Volume(r)) => self.volume_at(r, m.column),
                    // Seek on release, so dragging doesn't restart playback per cell.
                    Some(Drag::Seek(_)) | None => {}
                },
                MouseEventKind::Up(MouseButton::Left) => {
                    if let (Some(Drag::Seek(r)), Some(p)) = (self.drag.take(), &mut self.player) {
                        if m.column != r.x || m.row != r.y {
                            let frac = (m.column.saturating_sub(r.x)) as f64 / r.width.max(1) as f64;
                            p.seek_to(frac.clamp(0.0, 1.0) * p.duration());
                        }
                    }
                    self.drag = None;
                }
                // The wheel over the volume slider changes the volume.
                MouseEventKind::ScrollDown if self.bar_hit(m.column, m.row).is_some_and(|h| h.0) => self.change_volume(-0.05),
                MouseEventKind::ScrollUp if self.bar_hit(m.column, m.row).is_some_and(|h| h.0) => self.change_volume(0.05),
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

    /// Set the volume from a position on a slider (0-150%).
    fn volume_at(&mut self, r: Rect, x: u16) {
        let frac = ((x.saturating_sub(r.x) as f32 + 1.0) / r.width.max(1) as f32).clamp(0.0, 1.0);
        let v = if x <= r.x && frac < 0.1 { 0.0 } else { frac * 1.5 };
        self.change_volume(v - self.store.volume);
    }

    /// The player-bar control under (x, y): a volume slider, else a seek bar.
    fn bar_hit(&self, x: u16, y: u16) -> Option<(bool, Rect)> {
        let hit = |r: &Rect| x >= r.x && x < r.x + r.width && y == r.y;
        self.geo.vol.iter().find(|r| hit(r)).map(|r| (true, *r)).or_else(|| self.geo.seek.iter().find(|r| hit(r)).map(|r| (false, *r)))
    }

    fn click(&mut self, x: u16, y: u16) {
        let hit = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        // Dialogs draw over everything, the video's controls and bars too.
        if self.dialog.is_some() && !self.fullscreen {
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
        // Address-bar suggestions draw over the video's controls.
        let covered = self.geo.sugg.iter().any(|(r, _)| hit(r));
        if let Some(&(_, c)) = self.geo.ctl.iter().find(|(r, _)| hit(r)).filter(|_| !covered) {
            return self.control(c);
        }
        if self.geo.players.iter().any(hit) {
            self.show_controls();
        }
        match self.bar_hit(x, y) {
            Some((true, r)) => {
                self.drag = Some(Drag::Volume(r));
                return self.volume_at(r, x);
            }
            Some((false, r)) => {
                if let Some(p) = &mut self.player {
                    self.drag = Some(Drag::Seek(r));
                    let frac = (x.saturating_sub(r.x)) as f64 / r.width.max(1) as f64;
                    p.seek_to(frac.clamp(0.0, 1.0) * p.duration());
                    return;
                }
            }
            None => {}
        }
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

    /// Right-click a song: its menu.
    fn right_click(&mut self, x: u16, y: u16) {
        if self.dialog.take().is_some() || self.fullscreen {
            return;
        }
        let pr = self.geo.page;
        if x < pr.x || x >= pr.x + pr.width || y < pr.y || y >= pr.y + pr.height {
            return;
        }
        let (dx, dy) = (x - pr.x, (y - pr.y) as usize + self.page().scroll);
        let song = self.doc().and_then(|d| d.targets.iter().find(|t| dx >= t.x && dx < t.x + t.w && dy >= t.y && dy < t.y + t.h as usize).map(|t| t.act.clone())).and_then(|a| self.song_of(&a));
        if let Some(s) = song {
            self.open_song_menu(s, (x, y));
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
            self.show_controls();
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
            KeyCode::End => {
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
        if matches!(k.code, KeyCode::Char(' ' | ',' | '.' | '<' | '>' | '+' | '=' | '-' | '_' | 'M' | 'S' | 'n' | 'f' | '0'..='9')) && !ctrl {
            self.show_controls();
        }
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
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(0.05),
            KeyCode::Char('-') | KeyCode::Char('_') => self.change_volume(-0.05),
            KeyCode::Char('M') => self.toggle_mute(),
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
            KeyCode::Char('S') => self.cycle_captions(),
            KeyCode::Char('C') => {
                self.store.show_chat = !self.store.show_chat;
                self.store_changed();
                self.flash(if self.store.show_chat { "chat shown" } else { "chat hidden" });
            }
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
                    let music = v.music.then(|| self.music_now(&v.id));
                    let w = Watch { video: v, info: None, desc_open: false, comments: Vec::new(), comments_cont: None, comment_count: None, comments_loading: false, queue: Vec::new(), qpos: 0, rating: None, music };
                    self.navigate(Kind::Watch(Box::new(w)));
                }
            }
            KeyCode::Char('m') if !self.fullscreen => {
                let song = self.focused().and_then(|a| self.song_of(&a)).map(|s| (s, self.focus_cell())).or_else(|| match &self.page().kind {
                    // The music player's own song.
                    Kind::Watch(w) => w.music.as_ref().map(|m| (crate::music::Song { video: w.video.clone(), album: m.album.clone(), ..Default::default() }, None)),
                    _ => None,
                });
                if let Some((s, at)) = song {
                    let at = at.unwrap_or((self.geo.page.x + 2, self.geo.page.y + 1));
                    self.open_song_menu(s, at);
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
        self.draw_ui(f);
    }

    fn draw_ui(&mut self, f: &mut Frame) {
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
        if let Some((ax, ay)) = d.at {
            return self.draw_popup(buf, area, ax, ay);
        }
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

    /// A song menu, popped out from (ax, ay): leftward when there's no
    /// room to the right, upward when none below.
    fn draw_popup(&mut self, buf: &mut Buffer, area: Rect, ax: u16, ay: u16) {
        let Some(d) = &self.dialog else { return };
        let w = (d.rows.iter().map(|r| r.label.width()).max().unwrap_or(10) as u16 + 6).min(area.width);
        let rows = d.rows.len().min(area.height as usize - 2);
        let h = rows as u16 + 2;
        let right = area.x + area.width;
        let x = if ax + 1 + w <= right { ax + 1 } else { ax.saturating_sub(w).max(area.x) };
        let bottom = area.y + area.height;
        let y = if ay + 1 + h <= bottom { ay + 1 } else { ay.saturating_sub(h).max(area.y) };
        let bg = Color::Rgb(40, 40, 46);
        fill(buf, area, x, (y - area.y) as i64, w, h, bg);
        let border = Style::default().fg(Color::Rgb(90, 90, 100)).bg(bg);
        for cx in x + 1..x + w - 1 {
            buf[(cx, y)].set_char('─').set_style(border);
            buf[(cx, y + h - 1)].set_char('─').set_style(border);
        }
        for cy in y + 1..y + h - 1 {
            buf[(x, cy)].set_char('│').set_style(border);
            buf[(x + w - 1, cy)].set_char('│').set_style(border);
        }
        for (cx, cy, ch) in [(x, y, '╭'), (x + w - 1, y, '╮'), (x, y + h - 1, '╰'), (x + w - 1, y + h - 1, '╯')] {
            buf[(cx, cy)].set_char(ch).set_style(border);
        }
        let first = d.sel.saturating_sub(rows.saturating_sub(1));
        for (k, (i, row)) in d.rows.iter().enumerate().skip(first).take(rows).enumerate() {
            let line = Rect { x: x + 1, y: y + 1 + k as u16, width: w - 2, height: 1 };
            let style = if i == d.sel { Style::default().fg(Color::White).bg(Color::Rgb(75, 75, 85)) } else { Style::default().fg(Color::Rgb(230, 230, 230)).bg(bg) };
            buf.set_style(line, style);
            buf.set_string(x + 2, line.y, page::trunc(&row.label, w as usize - 4), style);
            self.geo.dlg.push((line, i));
        }
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
                let music = matches!(&self.page().kind, Kind::Music { .. }) || matches!(&self.page().kind, Kind::Watch(w) if w.music.is_some());
                let shown = if !e.text.is_empty() { e.text.clone() } else if music { "Search YouTube Music or paste a URL".to_string() } else { "Search YouTube or paste a URL".to_string() };
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
        let fixed = items.iter().take_while(|(_, s)| !matches!(s, Side::Channel(_))).count();
        let cur_kind = match &self.page().kind {
            Kind::Home { .. } => 0,
            Kind::Subs { .. } => 1,
            Kind::Music { .. } => 2,
            Kind::Watch(w) if w.music.is_some() => 2,
            Kind::History => 3,
            Kind::Later => 4,
            Kind::Playlists { .. } | Kind::MyPlaylist { .. } => 5,
            Kind::Help => 6,
            Kind::Account => 7,
            _ => usize::MAX,
        };
        let cur_channel = match &self.page().kind {
            Kind::Channel { id, .. } => Some(id.clone()),
            _ => None,
        };
        let mut y = r.y + 1;
        for (i, (label, item)) in items.iter().enumerate() {
            if i == fixed {
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
        let controls = self.controls_on();
        let has_next = self.next_video().is_some();
        let t = &mut self.tabs[self.cur];
        let page = &mut t.pages[t.idx];
        let key = (w, vh, page.rev, key_store);
        if page.layout.as_ref().map(|l| l.0) != Some(key) {
            let ctx = Ctx {
                autoplay,
                chat: show_chat,
                account: self.auth.as_ref().map(|a| a.channel()),
                yt_cookies: crate::cookies::status(),
                yt_account: self.account_name.clone(),
                capturing: self.capture.is_some(),
                logging_in: self.login.is_some(),
            };
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
                    let sy = *y as i64 - scroll as i64;
                    match self.thumbs.cells(url, iw, *h) {
                        Some(cells) => blit(buf, r, r.x + x, sy, cells, iw as usize, *h as usize),
                        None => fill(buf, r, r.x + x, sy, iw, *h, Color::Rgb(38, 38, 46)),
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
                    fill(buf, r, r.x + x, sy, pw, *h, Color::Rgb(0, 0, 0));
                    let mine = self.player.as_mut().filter(|p| &p.video.id == id);
                    match mine {
                        Some(p) => {
                            player_visible = sy + (*h as i64) / 2 >= 0 && sy + (*h as i64) / 2 < vh as i64;
                            match p.cells(&mut self.renderer, pw as usize, *h as usize) {
                                Some((cells, cw, ch, ox, oy)) => blit(buf, r, r.x + x + ox as u16, sy + oy as i64, cells, cw, ch),
                                None => center_text(buf, r, r.x + x, sy, pw, *h, " Loading video… "),
                            }
                            let ch = if controls { h.saturating_sub(1) } else { *h };
                            draw_captions(buf, r, r.x + x, sy, pw, ch, p.caption_text());
                            let top = (sy.max(0) as u16).min(r.height);
                            let bottom = ((sy + *h as i64).max(0) as u16).min(r.height);
                            self.geo.players.push(Rect { x: r.x + x, y: r.y + top, width: pw, height: bottom - top });
                            if controls {
                                let c = draw_controls(buf, r, r.x + x, sy, pw, *h, p, has_next, false);
                                self.geo.ctl.extend(c);
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
                                center_text(buf, r, r.x + x, sy, pw, *h, " Loading video… ");
                            } else {
                                let url = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
                                let (cw, ch, ox, oy) = fit(16.0 / 9.0, pw as usize, *h as usize);
                                if let Some(cells) = self.thumbs.cells(&url, cw as u16, ch as u16) {
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
                            let (seek, vol) = draw_bar(buf, r.x + x, r.y + (y - scroll) as u16, (*bw).min(w.saturating_sub(*x)), p, false);
                            self.geo.seek.push(seek);
                            self.geo.vol.extend(vol);
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
        fill(buf, r, r.x, 0, mw, vh, Color::Rgb(0, 0, 0));
        if let Some((cells, cw, ch, ox, oy)) = p.cells(&mut self.renderer, mw as usize, vh as usize) {
            blit(buf, r, r.x + ox as u16, oy as i64, cells, cw, ch);
        } else {
            center_text(buf, r, r.x, 0, mw, vh, "Loading…");
        }
        draw_captions(buf, r, r.x, 0, mw, vh, p.caption_text());
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
        let _ = draw_bar(buf, r.x, r.y + vh + 1, mw, p, true);
        self.geo.mini = Some(r);
        self.geo.mini_bar = Some(Rect { x: r.x, y: r.y + vh + 1, width: mw.saturating_sub(16), height: 1 });
    }

    fn draw_fullscreen(&mut self, buf: &mut Buffer, area: Rect) {
        let controls = self.controls_on();
        let has_next = self.next_video().is_some();
        let Some(p) = &mut self.player else { return };
        buf.set_style(area, Style::default().bg(Color::Rgb(0, 0, 0)));
        let vh = area.height - 1;
        let side = self.store.show_chat && self.chat.as_ref().is_some_and(|c| c.video == p.video.id) && area.width >= 100;
        let vw = if side { area.width - 44 } else { area.width };
        let varea = Rect { width: vw, ..area };
        match p.cells(&mut self.renderer, vw as usize, vh as usize) {
            Some((cells, cw, ch, ox, oy)) => blit(buf, varea, area.x + ox as u16, oy as i64, cells, cw, ch),
            None => center_text(buf, varea, area.x, 0, vw, vh, "Loading video…"),
        }
        draw_captions(buf, varea, area.x, 0, vw, if controls { vh - 1 } else { vh }, p.caption_text());
        self.geo.players.push(Rect { height: vh, ..varea });
        if controls {
            self.geo.ctl = draw_controls(buf, varea, area.x, 0, vw, vh, p, has_next, true);
        }
        if side {
            let inp = self.chat.as_ref().is_some_and(|c| !c.replay).then_some(self.auth.is_some());
            draw_chat(buf, area, area.x + vw + 1, 0, 43, vh, self.chat.as_ref(), inp);
        }
        let br = Rect { y: area.y + vh, height: 1, ..area };
        buf.set_style(br, Style::default().bg(Color::Rgb(40, 0, 0)));
        let (seek, vol) = draw_bar(buf, br.x, br.y, br.width, p, false);
        self.geo.seek.push(seek);
        self.geo.vol.extend(vol);
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
        let right = format!(" {audio} │ / search  ↵ open  H back  f full  +- vol  C chat  ? help ");
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

    pub fn run(&mut self, terminal: &mut Term, out: &crate::out::Out, rx: Receiver<Msg>) -> Result<()> {
        use crossterm::terminal::BeginSynchronizedUpdate;
        self.sync_subs(false);
        if self.api.signed_in() {
            self.fetch_account();
        }
        let mut last_save = Instant::now();
        let mut last_draw = Instant::now();
        let sync = std::env::var_os("YTUI_NO_SYNC").is_none();
        // A new video frame (or spinner step) not yet drawn.
        let mut owed = false;
        let budget = std::env::var_os("YTUI_NO_BUDGET").is_none();
        while !self.quit {
            while let Ok(m) = rx.try_recv() {
                self.on_msg(m);
            }
            let ticked = self.player.as_mut().is_some_and(|p| p.tick());
            // Spinners (resolving / loading) animate at ~30 fps, not as fast as
            // the loop spins.
            let spinning = (self.pending.is_some() || self.player.as_ref().is_some_and(|p| p.loading())) && last_draw.elapsed() >= Duration::from_millis(33);
            let fresh = ticked || spinning;
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
            owed |= fresh;
            // While the terminal is still working through the last frame,
            // newer frames replace each other here instead of queueing.
            if self.dirty || (owed && !out.busy()) {
                owed = false;
                last_draw = Instant::now();
                let t0 = Instant::now();
                if sync {
                    let _ = crossterm::queue!(terminal.backend_mut(), BeginSynchronizedUpdate);
                }
                // Size this frame's video to what the terminal can take.
                let fps = self.player.as_ref().map_or(0.0, |p| p.frame_rate());
                self.renderer.budget = (budget && fps > 0.0).then(|| (out.rate() * crate::out::BUDGET_SHARE / fps) as usize);
                terminal.draw(|f| self.draw(f))?;
                let t1 = Instant::now();
                if sync {
                    self.frame_out.extend_from_slice(b"\x1b[?2026l");
                }
                // The whole frame goes out as one write on the output thread;
                // this waits only for the previous frame to be taken.
                out.push(&self.frame_out);
                self.frame_out.clear();
                self.dirty = false;
                if let Some(s) = &mut self.stats {
                    let p = self.player.as_ref();
                    s.frame(t1 - t0, t1.elapsed(), p.map_or(0.0, |p| p.fps), p.map_or(0.0, |p| p.pos), format!("{} | tol {} rate {:.1}MB/s | {}", p.map(|p| p.debug()).unwrap_or_default(), self.renderer.tol, out.rate() / 1e6, crate::sys::console_state()));
                }
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
        out.drain();
        Ok(())
    }
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
            buf[(cx, clip.y + y as u16)].set_char(c.ch).set_fg(Color::Rgb(c.fg[0], c.fg[1], c.fg[2])).set_bg(Color::Rgb(c.bg[0], c.bg[1], c.bg[2]));
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

/// Per-second frame timing, appended to the YTUI_STATS file.
struct Stats {
    path: String,
    since: Instant,
    n: u32,
    draw: Duration,
    write: Duration,
    write_max: Duration,
}

impl Stats {
    fn new(path: String) -> Stats {
        let _ = std::fs::write(&path, "");
        Stats { path, ..Stats::new_keep() }
    }

    fn frame(&mut self, draw: Duration, write: Duration, decoded_fps: f64, pos: f64, dbg: String) {
        self.n += 1;
        self.draw += draw;
        self.write += write;
        self.write_max = self.write_max.max(write);
        let el = self.since.elapsed().as_secs_f64();
        if el >= 1.0 {
            let n = self.n.max(1) as f64;
            let line = format!(
                "pos {pos:7.2}  drawn {:5.1} fps  decoded {decoded_fps:5.1} fps  ui+text {:6.2} ms  write {:6.2} ms (max {:6.1})  {dbg}\n",
                self.n as f64 / el,
                self.draw.as_secs_f64() * 1000.0 / n,
                self.write.as_secs_f64() * 1000.0 / n,
                self.write_max.as_secs_f64() * 1000.0,
            );
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&self.path) {
                let _ = f.write_all(line.as_bytes());
            }
            *self = Stats { path: std::mem::take(&mut self.path), ..Stats::new_keep() };
        }
    }

    fn new_keep() -> Stats {
        Stats { path: String::new(), since: Instant::now(), n: 0, draw: Duration::ZERO, write: Duration::ZERO, write_max: Duration::ZERO }
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

/// Captions over the bottom of a w x h video box (top-left at column x,
/// row sy of `clip`): centred, wrapped lines in bold white on a solid black
/// box with a blank row above and below, so they read over any picture.
fn draw_captions(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, w: u16, h: u16, text: &str) {
    if text.trim().is_empty() || w < 8 || h < 4 {
        return;
    }
    let max = (w as usize * 8 / 10).max(6);
    let mut lines: Vec<String> = Vec::new();
    for para in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if !cur.is_empty() && cur.width() + 1 + word.width() > max {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            lines.push(page::trunc(&cur, max));
        }
    }
    // Newest lines win when there are more than fit.
    let keep = (h as usize / 4).clamp(1, 3);
    let lines = &lines[lines.len().saturating_sub(keep)..];
    let pad = h >= 12;
    let bw = (lines.iter().map(|l| l.width()).max().unwrap_or(0) + 4).min(w as usize) as u16;
    let bx = x + w.saturating_sub(bw) / 2;
    let rows = lines.len() as i64 + 2 * pad as i64;
    let bottom = sy + h as i64 - 1 - (h >= 16) as i64;
    let style = Style::default().fg(Color::Rgb(255, 255, 255)).bg(Color::Rgb(0, 0, 0)).add_modifier(Modifier::BOLD);
    for r in 0..rows {
        let y = bottom - (rows - 1 - r);
        if y < 0 || y >= clip.height as i64 {
            continue;
        }
        let yy = clip.y + y as u16;
        for xx in bx..bx + bw {
            buf[(xx, yy)].set_char(' ').set_style(style);
        }
        if let Some(l) = usize::try_from(r - pad as i64).ok().and_then(|i| lines.get(i)) {
            buf.set_string(x + w.saturating_sub(l.width() as u16) / 2, yy, l, style);
        }
    }
}

#[derive(Clone, Copy)]
enum Drag {
    Volume(Rect),
    Seek(Rect),
}

/// Controls over the bottom row of a w x h video box (top-left at column x,
/// row sy of `clip`), like YouTube's: previous, play/pause, next and volume
/// on the left; captions and fullscreen on the right. (The time is on the
/// player bar.) Returns the
/// buttons drawn.
fn draw_controls(buf: &mut Buffer, clip: Rect, x: u16, sy: i64, w: u16, h: u16, p: &Player, has_next: bool, full: bool) -> Vec<(Rect, Ctl)> {
    let y = sy + h as i64 - 1;
    if w < 24 || h < 3 || y < 0 || y >= clip.height as i64 {
        return Vec::new();
    }
    let y = clip.y + y as u16;
    let bg = Color::Rgb(0, 0, 0);
    let on = Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD);
    let off = Style::default().fg(Color::Rgb(110, 110, 120)).bg(bg);
    buf.set_style(Rect { x, y, width: w, height: 1 }, Style::default().bg(bg));
    for xx in x..x + w {
        buf[(xx, y)].set_char(' ');
    }
    let pause = if p.ended { " ↻ " } else if p.paused || p.loading() { " ▶ " } else { " ❚❚ " };
    let vol = if p.muted() { " muted ".to_string() } else { format!(" vol {:.0}% ", p.level() * 100.0) };
    let left = [(" |◀ ".to_string(), Ctl::Prev, true), (pause.to_string(), Ctl::Pause, true), (" ▶| ".to_string(), Ctl::Next, has_next), (vol, Ctl::Mute, true)];
    let cc = p.caption_track().is_some();
    let right = [(" CC ", Ctl::Captions, cc), (if full { " exit " } else { " full " }, Ctl::Full, true)];
    let rw: u16 = right.iter().map(|r| r.0.width() as u16 + 1).sum();
    let mut out = Vec::new();
    let mut cx = x + 1;
    for (label, c, lit) in left {
        let lw = label.width() as u16;
        if cx + lw + rw >= x + w {
            break;
        }
        buf.set_string(cx, y, &label, if lit { on } else { off });
        out.push((Rect { x: cx, y, width: lw, height: 1 }, c));
        cx += lw;
    }
    let mut rx = x + w - rw;
    for (label, c, lit) in right {
        let lw = label.width() as u16;
        // Captions on: underlined in the accent colour, like YouTube's red bar.
        let st = if c == Ctl::Captions && lit { on.underline_color(Color::Rgb(255, 0, 0)).add_modifier(Modifier::UNDERLINED) } else if lit { on } else { off };
        buf.set_string(rx, y, label, st);
        out.push((Rect { x: rx, y, width: lw, height: 1 }, c));
        rx += lw + 1;
    }
    out
}

/// Cells in the volume slider (0-150%).
const VOL_CELLS: u16 = 12;

/// The player bar: seek bar, time, and (full size) a volume slider.
/// Returns where the seek bar and the volume slider were drawn.
fn draw_bar(buf: &mut Buffer, x: u16, y: u16, w: u16, p: &Player, compact: bool) -> (Rect, Option<Rect>) {
    let d = p.duration();
    let icon = if p.loading() { "…" } else if p.ended { "■" } else if p.paused { "❚❚" } else { "▶" };
    let time = if p.stream.live { format!("{icon} LIVE {}", fmt_time(p.pos)) } else { format!("{icon} {} / {}", fmt_time(p.pos), fmt_time(d)) };
    let pct = if p.muted() { "muted".to_string() } else { format!("{:>3.0}%", p.level() * 100.0) };
    let ae = if p.exposure.on { " AE" } else { "" };
    let cc = if p.caption_track().is_some() { " CC" } else { "" };
    let rate = format!("{:.0}fps", p.fps);
    // [time]  vol [slider] pct  [AE CC rate]
    let head = format!(" {time}  vol ");
    let rest = format!(" {pct} {ae}{cc} {rate} ");
    let slider = !compact && w > head.width() as u16 + VOL_CELLS + rest.width() as u16 + 16;
    let tail = if compact || !slider { format!(" {time} ") } else { String::new() };
    let tw = if slider { head.width() as u16 + VOL_CELLS + rest.width() as u16 } else { tail.width() as u16 };
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
    let seek = Rect { x, y, width: bw.max(1), height: 1 };
    let tx = x + w.saturating_sub(tw);
    if !slider {
        buf.set_string(tx, y, &tail, Style::default().fg(Color::White));
        return (seek, None);
    }
    let white = Style::default().fg(Color::White);
    buf.set_string(tx, y, &head, white);
    let sx = tx + head.width() as u16;
    let lit = if p.muted() { 0 } else { ((p.level() / 1.5) * VOL_CELLS as f32).round() as u16 };
    for i in 0..VOL_CELLS {
        // Over 100% shows in a warmer colour, like a boost.
        let over = i >= (VOL_CELLS * 2) / 3;
        let c = &mut buf[(sx + i, y)];
        if i < lit {
            c.set_char('█').set_fg(if over { Color::Rgb(255, 150, 60) } else { Color::Rgb(235, 235, 235) });
        } else {
            c.set_char('▁').set_fg(Color::Rgb(90, 90, 100));
        }
    }
    buf.set_string(sx + VOL_CELLS, y, &rest, white);
    (seek, Some(Rect { x: sx, y, width: VOL_CELLS, height: 1 }))
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
