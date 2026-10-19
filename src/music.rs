// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

//! YouTube Music (music.youtube.com): its own API client (the WEB_REMIX
//! web client) and one page model for everything it serves. Home, Explore,
//! charts, new releases, moods and genres, search results, artists, albums,
//! playlists and the library are all a header plus a list of sections, each
//! section a row of cards, a list of songs or a set of genre buttons; one
//! parser reads them all.

use crate::api::{find, find_all, last_url, text, Api, VideoItem};
use anyhow::Result;
use serde_json::{json, Value};

/// Where a music page points: a browse id (artist, album, playlist, chart,
/// genre, library shelf...) with optional params.
#[derive(Clone, Debug, PartialEq)]
pub struct Browse {
    pub id: String,
    pub params: Option<String>,
}

impl Browse {
    pub fn new(id: &str) -> Browse {
        Browse { id: id.into(), params: None }
    }
}

/// Search filters, as YouTube Music's chips.
pub const FILTERS: &[(&str, Option<&str>)] = &[
    ("All", None),
    ("Songs", Some("EgWKAQIIAWoSEAUQCRADEAoQBBAQEA4QFRAR")),
    ("Videos", Some("EgWKAQIQAWoSEAUQCRADEAoQBBAQEA4QFRAR")),
    ("Albums", Some("EgWKAQIYAWoSEAUQCRADEAoQBBAQEA4QFRAR")),
    ("Artists", Some("EgWKAQIgAWoSEAUQCRADEAoQBBAQEA4QFRAR")),
    ("Playlists", Some("EgeKAQQoADgBahIQBRAJEAMQChAEEBAQDhAVEBE%3D")),
    ("Community playlists", Some("EgeKAQQoAEABahIQBRAJEAMQChAEEBAQDhAVEBE%3D")),
    ("Podcasts", Some("EgWKAQJQAWoSEAUQCRADEAoQBBAQEA4QFRAR")),
];

/// The library's shelves (signed in).
pub const LIBRARY: &[(&str, &str)] = &[
    ("Playlists", "FEmusic_liked_playlists"),
    ("Liked songs", "VLLM"),
    ("Albums", "FEmusic_liked_albums"),
    ("Artists", "FEmusic_library_corpus_track_artists"),
    ("Subscriptions", "FEmusic_library_corpus_artists"),
    ("History", "FEmusic_history"),
];

/// A section of a music page.
#[derive(Clone, Debug, Default)]
pub struct Section {
    pub title: String,
    /// "More" link of the section, if any.
    pub more: Option<Browse>,
    pub items: Vec<MItem>,
    /// Text-only section (an album's or artist's description).
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub enum MItem {
    /// A song or music video that plays.
    Song(Song),
    /// An album, playlist, artist, chart or video shown as a card.
    Card(Card),
    /// A mood or genre button.
    Nav { label: String, to: Browse, color: Option<u32> },
}

#[derive(Clone, Debug, Default)]
pub struct Song {
    pub video: VideoItem,
    pub album: Option<(String, String)>,
    pub thumb: Option<String>,
    /// Playlist it plays in (album / playlist pages), for the queue.
    pub playlist: Option<String>,
    /// Row number shown on album tracks and charts.
    pub index: Option<String>,
    /// Signed in, the menu's library toggle: (label, feedback token), the
    /// one to show first.
    pub library: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub title: String,
    pub subtitle: String,
    pub thumb: Option<String>,
    pub kind: CardKind,
    /// Artists are drawn round in YouTube Music; here, with a caption.
    pub artist: bool,
}

#[derive(Clone, Debug)]
pub enum CardKind {
    Open(Browse),
    /// A video (or a playlist's first video) to play.
    Play { video: VideoItem, playlist: Option<String> },
}

/// Page header: artist, album or playlist.
#[derive(Clone, Debug, Default)]
pub struct Header {
    pub title: String,
    pub subtitle: String,
    pub second: String,
    pub description: String,
    pub thumb: Option<String>,
    /// Artist pages carry a wide banner instead of square art.
    pub banner: bool,
    /// Playlist to play from the header's Play button.
    pub play: Option<String>,
    /// Shuffle / radio playlists.
    pub shuffle: Option<String>,
    pub radio: Option<String>,
    /// The artist named in an album's header, to link to.
    pub artist: Option<(String, String)>,
    pub subscribers: Option<String>,
    /// Channel id to subscribe to (artist pages).
    pub channel: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MusicPage {
    pub header: Option<Header>,
    pub sections: Vec<Section>,
    pub cont: Option<String>,
}

impl MusicPage {
    /// Every song on the page, in order (the queue a song plays in).
    pub fn songs(&self) -> Vec<Song> {
        self.sections.iter().flat_map(|s| s.items.iter()).filter_map(|i| if let MItem::Song(s) = i { Some(s.clone()) } else { None }).collect()
    }

    pub fn extend(&mut self, more: MusicPage) {
        // A shelf's own continuation (a long playlist) adds to its last list.
        let mut secs = more.sections.into_iter();
        if let Some(first) = secs.next() {
            match self.sections.last_mut() {
                Some(last) if first.title.is_empty() && first.items.iter().all(|i| matches!(i, MItem::Song(_))) && last.items.iter().any(|i| matches!(i, MItem::Song(_))) => {
                    last.items.extend(first.items)
                }
                _ => self.sections.push(first),
            }
        }
        self.sections.extend(secs);
        self.cont = more.cont;
    }
}

/// What's playing next: the queue, and the Lyrics / Related tabs.
#[derive(Clone, Debug, Default)]
pub struct Next {
    pub queue: Vec<Song>,
    pub lyrics: Option<String>,
    pub related: Option<String>,
}

/// Lyrics text and where they come from ("Source: Musixmatch").
#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub text: String,
    pub source: String,
}

impl Api {
    pub fn music_browse(&self, b: &Browse) -> Result<MusicPage> {
        let mut body = json!({"browseId": b.id});
        if let Some(p) = &b.params {
            body["params"] = json!(p);
        }
        Ok(page_from(&self.music_post("browse", body)?))
    }

    pub fn music_search(&self, q: &str, params: Option<&str>) -> Result<MusicPage> {
        let mut body = json!({"query": q});
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        Ok(page_from(&self.music_post("search", body)?))
    }

    /// Search's "All": the top result, then the first few songs, videos,
    /// albums, artists and playlists, each section's More opening its filter.
    pub fn music_search_all(&self, q: &str) -> Result<MusicPage> {
        let mut page = self.music_search(q, None)?;
        page.sections.retain(|s| s.title == "Top result");
        let kinds: Vec<(usize, &str)> = [1, 2, 3, 4, 5].into_iter().map(|i| (i, FILTERS[i].0)).collect();
        let found: Vec<(usize, &str, Result<MusicPage>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = kinds.iter().map(|&(i, name)| (i, name, sc.spawn(move || self.music_search(q, FILTERS[i].1)))).collect();
            hs.into_iter().map(|(i, name, h)| (i, name, h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("search failed"))))).collect()
        });
        for (i, name, r) in found {
            let Ok(p) = r else { continue };
            let items: Vec<MItem> = p.sections.into_iter().flat_map(|s| s.items).take(if i == 1 { 6 } else { 8 }).collect();
            if !items.is_empty() {
                page.sections.push(Section { title: name.into(), more: Some(Browse { id: format!("filter:{i}"), params: None }), items, text: None });
            }
        }
        page.cont = None;
        Ok(page)
    }

    /// The next page of a browse or search (`search` picks the endpoint).
    pub fn music_more(&self, token: &str, search: bool) -> Result<MusicPage> {
        Ok(page_from(&self.music_post(if search { "search" } else { "browse" }, json!({"continuation": token}))?))
    }

    /// The queue for a song: its playlist, or a radio started from it.
    pub fn music_next(&self, video: Option<&str>, playlist: Option<&str>) -> Result<Next> {
        let pl = playlist.map(String::from).unwrap_or_else(|| format!("RDAMVM{}", video.unwrap_or_default()));
        let mut body = json!({"playlistId": pl, "isAudioOnly": true});
        // A song's mix: the "params" YouTube Music's own Start mix sends.
        // Without it the answer is the song's fixed radio, the same queue
        // every time.
        if pl.starts_with("RDAMVM") {
            body["params"] = json!("wAEB");
        }
        if let Some(v) = video {
            body["videoId"] = json!(v);
        }
        let v = self.music_post("next", body)?;
        let mut n = Next::default();
        let mut panels = Vec::new();
        queue_panels(&v["contents"], &mut panels);
        for p in panels {
            if let Some(s) = panel_song(p, &pl) {
                if !n.queue.iter().any(|q: &Song| q.video.id == s.video.id) {
                    n.queue.push(s);
                }
            }
        }
        let mut tabs = Vec::new();
        find_all(&v["contents"], "tabRenderer", &mut tabs);
        for t in tabs {
            let id = t["endpoint"]["browseEndpoint"]["browseId"].as_str().map(String::from);
            match t["title"].as_str() {
                Some("Lyrics") => n.lyrics = id,
                Some("Related") => n.related = id,
                _ => {}
            }
        }
        Ok(n)
    }

    /// Send a menu's feedback token (library add / remove).
    pub fn music_feedback(&self, token: &str) -> Result<()> {
        let v = self.music_post("feedback", json!({"feedbackTokens": [token]}))?;
        match v["feedbackResponses"][0]["isProcessed"].as_bool() {
            Some(true) => Ok(()),
            _ => anyhow::bail!("YouTube Music didn't take that"),
        }
    }

    /// Like a song on the signed-in account (its Liked songs).
    pub fn music_like(&self, id: &str) -> Result<()> {
        self.music_post("like/like", json!({"target": {"videoId": id}}))?;
        Ok(())
    }

    /// A song's credits: (role, names) pairs ("Written by", "Kevin MacLeod").
    pub fn music_credits(&self, id: &str) -> Result<Vec<(String, String)>> {
        let v = self.music_post("browse", json!({"browseId": format!("MPTC{id}")}))?;
        let mut secs = Vec::new();
        find_all(&v, "dismissableDialogContentSectionRenderer", &mut secs);
        let out: Vec<(String, String)> = secs.iter().filter_map(|s| Some((text(&s["title"])?, text(&s["subtitle"]).unwrap_or_default()))).collect();
        if out.is_empty() {
            anyhow::bail!("YouTube Music has no credits for this one");
        }
        Ok(out)
    }

    pub fn music_lyrics(&self, id: &str) -> Result<Lyrics> {
        let v = self.music_post("browse", json!({"browseId": id}))?;
        let shelf = find(&v["contents"], "musicDescriptionShelfRenderer");
        match shelf {
            Some(s) => Ok(Lyrics { text: text(&s["description"]).unwrap_or_default(), source: text(&s["footer"]).unwrap_or_default() }),
            None => anyhow::bail!("{}", find(&v["contents"], "messageRenderer").and_then(|m| text(&m["text"])).unwrap_or_else(|| "Lyrics aren't available for this song".into())),
        }
    }
}

/// Page type of a browse endpoint ("MUSIC_PAGE_TYPE_ALBUM"...).
fn page_type(ep: &Value) -> &str {
    ep["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]["pageType"].as_str().unwrap_or("")
}

fn browse_of(ep: &Value) -> Option<Browse> {
    let id = ep["browseId"].as_str()?;
    Some(Browse { id: id.into(), params: ep["params"].as_str().map(String::from) })
}

/// Artists and album named in a run list ("Artist • Album • 3:08").
struct Runs {
    artists: Vec<(String, Option<String>)>,
    album: Option<(String, String)>,
    duration: Option<String>,
    other: Vec<String>,
}

fn is_duration(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.split(':').count() >= 2 && t.chars().all(|c| c.is_ascii_digit() || c == ':')
}

fn runs(v: &Value) -> Runs {
    let mut r = Runs { artists: Vec::new(), album: None, duration: None, other: Vec::new() };
    for run in v["runs"].as_array().into_iter().flatten() {
        let t = run["text"].as_str().unwrap_or("").to_string();
        if t.trim().is_empty() || t.trim() == "•" || t.trim() == "&" || t.trim() == "," {
            continue;
        }
        let ep = &run["navigationEndpoint"]["browseEndpoint"];
        match page_type(ep) {
            "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL" => r.artists.push((t, ep["browseId"].as_str().map(String::from))),
            "MUSIC_PAGE_TYPE_ALBUM" => r.album = ep["browseId"].as_str().map(|id| (t, id.to_string())),
            _ if is_duration(&t) => r.duration = Some(t),
            _ => r.other.push(t),
        }
    }
    r
}

fn thumb(v: &Value) -> Option<String> {
    last_url(v)
}

fn join_artists(a: &[(String, Option<String>)]) -> String {
    a.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(", ")
}

/// A row of a song list (album tracks, playlists, charts, search results)
/// or a browse row (an artist or album in search results).
fn list_item(x: &Value, playlist: Option<&str>) -> Option<MItem> {
    let cols: Vec<&Value> = x["flexColumns"].as_array().map(|a| a.iter().map(|c| &c["musicResponsiveListItemFlexColumnRenderer"]["text"]).collect()).unwrap_or_default();
    let title = cols.first().and_then(|c| text(c)).unwrap_or_default();
    // Rows that open a page (albums, artists, playlists in search) are
    // cards even though they carry a play button.
    let opens = x["navigationEndpoint"]["browseEndpoint"].is_object();
    let vid = x["playlistItemData"]["videoId"].as_str().or_else(|| find(&x["overlay"], "watchEndpoint").and_then(|w| w["videoId"].as_str())).filter(|v| !v.is_empty() && !opens);
    let mut meta = Runs { artists: Vec::new(), album: None, duration: None, other: Vec::new() };
    for c in cols.iter().skip(1) {
        let r = runs(c);
        meta.artists.extend(r.artists);
        meta.album = meta.album.or(r.album);
        meta.duration = meta.duration.or(r.duration);
        meta.other.extend(r.other);
    }
    for c in x["fixedColumns"].as_array().into_iter().flatten() {
        if let Some(t) = text(&c["musicResponsiveListItemFixedColumnRenderer"]["text"]) {
            if is_duration(&t) {
                meta.duration = Some(t);
            }
        }
    }
    let art = thumb(&x["thumbnail"]);
    if let Some(id) = vid {
        // Search rows lead with the type ("Song", "Video", "Episode").
        let kind = meta.other.first().filter(|t| ["Song", "Video", "Episode"].contains(&t.as_str())).cloned();
        let plays = meta.other.iter().find(|t| t.ends_with("plays") || t.ends_with("views")).cloned();
        let mut artists = meta.artists.clone();
        if artists.is_empty() {
            // Unlinked artist names (playlists of uploads) are plain runs.
            if let Some(a) = meta.other.iter().find(|t| Some(*t) != kind.as_ref() && Some(*t) != plays.as_ref()) {
                artists.push((a.clone(), None));
            }
        }
        let pl = find(&x["overlay"], "watchEndpoint").and_then(|w| w["playlistId"].as_str()).map(String::from).or_else(|| playlist.map(String::from));
        let video = VideoItem {
            id: id.into(),
            title,
            channel: join_artists(&artists),
            channel_id: artists.iter().find_map(|a| a.1.clone()),
            duration: meta.duration,
            views: plays.or(kind.filter(|k| k != "Song")),
            music: true,
            ..Default::default()
        };
        let index = text(&x["index"]).filter(|i| !i.is_empty()).or_else(|| find(&x["customIndexColumn"], "text").and_then(text).filter(|i| !i.is_empty()));
        return Some(MItem::Song(Song { video, album: meta.album, thumb: art, playlist: pl, index, library: library_of(&x["menu"]) }));
    }
    let ep = &x["navigationEndpoint"]["browseEndpoint"];
    let to = browse_of(ep)?;
    let artist = page_type(ep) == "MUSIC_PAGE_TYPE_ARTIST";
    let mut sub: Vec<String> = meta.other;
    if !meta.artists.is_empty() {
        sub.push(join_artists(&meta.artists));
    }
    Some(MItem::Card(Card { title, subtitle: sub.join(" • "), thumb: art, kind: CardKind::Open(to), artist }))
}

/// A card in a carousel or grid.
fn two_row(x: &Value) -> Option<MItem> {
    let title = text(&x["title"]).unwrap_or_default();
    let subtitle = text(&x["subtitle"]).unwrap_or_default();
    let art = thumb(&x["thumbnailRenderer"]);
    let nav = &x["navigationEndpoint"];
    if let Some(b) = browse_of(&nav["browseEndpoint"]) {
        let artist = page_type(&nav["browseEndpoint"]) == "MUSIC_PAGE_TYPE_ARTIST";
        return Some(MItem::Card(Card { title, subtitle, thumb: art, kind: CardKind::Open(b), artist }));
    }
    let w = &nav["watchEndpoint"];
    let id = w["videoId"].as_str()?;
    let r = runs(&x["subtitle"]);
    let video = VideoItem {
        id: id.into(),
        title: title.clone(),
        channel: join_artists(&r.artists),
        channel_id: r.artists.iter().find_map(|a| a.1.clone()),
        views: r.other.iter().find(|t| t.ends_with("views") || t.ends_with("plays")).cloned(),
        music: true,
        ..Default::default()
    };
    Some(MItem::Card(Card { title, subtitle, thumb: art, kind: CardKind::Play { video, playlist: w["playlistId"].as_str().map(String::from) }, artist: false }))
}

fn nav_button(x: &Value) -> Option<MItem> {
    let to = browse_of(&x["clickCommand"]["browseEndpoint"])?;
    let color = x["solid"]["leftStripeColor"].as_u64().map(|c| (c & 0xffffff) as u32);
    Some(MItem::Nav { label: text(&x["buttonText"]).unwrap_or_default(), to, color })
}

/// The queue entries of a "next" response. Signed in, each entry is a
/// wrapper holding the song and its "counterpart" (the music video for an
/// audio track, or the other way round); only the song itself is kept, or
/// the queue would play every song twice.
fn queue_panels<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match k.as_str() {
                    "playlistPanelVideoRenderer" => out.push(x),
                    "counterpart" => {}
                    _ => queue_panels(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| queue_panels(x, out)),
        _ => {}
    }
}

/// One queue entry of the "next" endpoint.
fn panel_song(p: &Value, playlist: &str) -> Option<Song> {
    let id = p["videoId"].as_str()?;
    let r = runs(&p["longBylineText"]);
    let artists = if r.artists.is_empty() { text(&p["shortBylineText"]).map(|a| vec![(a, None)]).unwrap_or_default() } else { r.artists };
    let video = VideoItem {
        id: id.into(),
        title: text(&p["title"]).unwrap_or_default(),
        channel: join_artists(&artists),
        channel_id: artists.iter().find_map(|a| a.1.clone()),
        duration: text(&p["lengthText"]),
        music: true,
        ..Default::default()
    };
    Some(Song { video, album: r.album, thumb: thumb(&p["thumbnail"]), playlist: Some(playlist.into()), index: None, library: library_of(&p["menu"]) })
}

/// A song menu's "Save to library" / "Remove from library" toggle, signed
/// in: (label, token) for what it does now, then for the other way.
fn library_of(menu: &Value) -> Vec<(String, String)> {
    let Some(t) = find(menu, "toggleMenuServiceItemRenderer") else { return Vec::new() };
    let tok = |ep: &Value| ep["feedbackEndpoint"]["feedbackToken"].as_str().map(String::from);
    match (text(&t["defaultText"]), tok(&t["defaultServiceEndpoint"]), text(&t["toggledText"]), tok(&t["toggledServiceEndpoint"])) {
        (Some(a), Some(ta), Some(b), Some(tb)) => vec![(a, ta), (b, tb)],
        _ => Vec::new(),
    }
}

fn items_of(list: &Value, playlist: Option<&str>) -> Vec<MItem> {
    let mut out = Vec::new();
    for c in list.as_array().into_iter().flatten() {
        let it = if let Some(x) = c.get("musicResponsiveListItemRenderer") {
            list_item(x, playlist)
        } else if let Some(x) = c.get("musicTwoRowItemRenderer") {
            two_row(x)
        } else if let Some(x) = c.get("musicNavigationButtonRenderer") {
            nav_button(x)
        } else if let Some(x) = c.get("musicMultiRowListItemRenderer") {
            // Podcast episodes.
            find(&x["onTap"], "watchEndpoint").and_then(|w| w["videoId"].as_str()).map(|id| {
                MItem::Song(Song {
                    video: VideoItem { id: id.into(), title: text(&x["title"]).unwrap_or_default(), channel: text(&x["subtitle"]).unwrap_or_default(), music: true, ..Default::default() },
                    thumb: thumb(&x["thumbnail"]),
                    ..Default::default()
                })
            })
        } else {
            None
        };
        out.extend(it);
    }
    out
}

fn section_title(s: &Value) -> (String, Option<Browse>) {
    let h = &s["header"];
    let basic = find(h, "title").or_else(|| s.get("title"));
    // A playlist's header row is its sort menu, not a title.
    let title = basic.and_then(text).filter(|t| t != "Sort").unwrap_or_default();
    let more = find(h, "moreContentButton")
        .and_then(|b| find(b, "browseEndpoint"))
        .and_then(browse_of)
        .or_else(|| basic.and_then(|t| find(t, "browseEndpoint")).and_then(browse_of));
    (title, more)
}

fn header_from(h: &Value) -> Option<Header> {
    if h.is_null() {
        return None;
    }
    let mut out = Header { title: text(&h["title"]).unwrap_or_default(), ..Default::default() };
    if out.title.is_empty() {
        return None;
    }
    out.subtitle = text(&h["subtitle"]).unwrap_or_default();
    out.second = text(&h["secondSubtitle"]).unwrap_or_default();
    out.description = find(&h["description"], "description").and_then(text).or_else(|| text(&h["description"])).unwrap_or_default();
    let strap = runs(&h["straplineTextOne"]);
    out.artist = strap.artists.first().and_then(|a| a.1.clone().map(|id| (a.0.clone(), id)));
    if out.subtitle.is_empty() {
        out.subtitle = text(&h["straplineTextOne"]).unwrap_or_default();
    }
    out.banner = h.get("musicImmersiveHeaderRenderer").is_some() || h["thumbnail"].get("musicThumbnailRenderer").is_some() && h.get("playButton").is_some() && h.get("startRadioButton").is_some();
    out.thumb = thumb(&h["thumbnail"]).or_else(|| thumb(&h["foregroundThumbnail"]));
    let mut eps = Vec::new();
    find_all(h, "watchPlaylistEndpoint", &mut eps);
    find_all(h, "watchEndpoint", &mut eps);
    let pls: Vec<String> = eps.iter().filter_map(|e| e["playlistId"].as_str().map(String::from)).collect();
    out.play = find(&h["playButton"], "watchEndpoint").and_then(|w| w["playlistId"].as_str().map(String::from)).or_else(|| pls.first().cloned());
    out.radio = find(&h["startRadioButton"], "watchPlaylistEndpoint").and_then(|w| w["playlistId"].as_str().map(String::from));
    out.shuffle = pls.iter().find(|p| Some(*p) != out.play.as_ref() && Some(*p) != out.radio.as_ref()).cloned();
    if let Some(sub) = find(h, "subscribeButtonRenderer") {
        out.subscribers = text(&sub["subscriberCountText"]).or_else(|| text(&sub["longSubscriberCountText"]));
        out.channel = sub["channelId"].as_str().map(String::from);
    }
    Some(out)
}

/// The page's continuation token.
fn cont_of(v: &Value) -> Option<String> {
    find(v, "nextContinuationData").and_then(|c| c["continuation"].as_str()).or_else(|| find(v, "continuationCommand").and_then(|c| c["token"].as_str())).map(String::from)
}

/// Read any browse / search response (or a continuation of one).
pub fn page_from(v: &Value) -> MusicPage {
    let mut page = MusicPage::default();
    // Headers live at the top (artists, moods) or as the first section
    // (albums, playlists).
    if let Some((_, h)) = v["header"].as_object().and_then(|m| m.iter().next()) {
        page.header = header_from(h).map(|mut hd| {
            hd.banner = v["header"].get("musicImmersiveHeaderRenderer").is_some() || v["header"].get("musicVisualHeaderRenderer").is_some();
            hd
        });
    }
    if page.header.is_none() {
        page.header = find(&v["contents"], "musicResponsiveHeaderRenderer").or_else(|| find(&v["contents"], "musicEditablePlaylistDetailHeaderRenderer").and_then(|e| find(e, "musicResponsiveHeaderRenderer"))).and_then(header_from);
    }
    let roots = [&v["contents"], &v["continuationContents"], &v["onResponseReceivedActions"]];
    let playlist = page.header.as_ref().and_then(|h| h.play.clone());
    for root in roots {
        collect(root, &mut page, playlist.as_deref());
    }
    // Header descriptions arrive as a description shelf on artist pages.
    if let Some(h) = &mut page.header {
        if h.description.is_empty() {
            if let Some(d) = page.sections.iter().find_map(|s| s.text.clone()) {
                h.description = d;
                page.sections.retain(|s| s.text.is_none());
            }
        }
    }
    page.cont = cont_of(&v["contents"]).or_else(|| cont_of(&v["continuationContents"])).or_else(|| cont_of(&v["onResponseReceivedActions"]));
    page
}

fn collect(v: &Value, page: &mut MusicPage, playlist: Option<&str>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match k.as_str() {
                    "musicCarouselShelfRenderer" | "musicImmersiveCarouselShelfRenderer" | "gridRenderer" | "musicShelfRenderer" | "musicPlaylistShelfRenderer"
                    | "musicShelfContinuation" | "musicPlaylistShelfContinuation" | "gridContinuation" => {
                        let (title, more) = section_title(x);
                        let list = if x["contents"].is_array() { &x["contents"] } else { &x["items"] };
                        let pl = x["playlistId"].as_str().or(playlist);
                        let items = items_of(list, pl);
                        if !items.is_empty() {
                            page.sections.push(Section { title, more, items, text: None });
                        }
                    }
                    "musicCardShelfRenderer" => {
                        // Search's top result: the card itself, then its songs.
                        let title = text(&x["title"]).unwrap_or_default();
                        let mut items = Vec::new();
                        let ep = &x["title"]["runs"][0]["navigationEndpoint"];
                        if let Some(b) = browse_of(&ep["browseEndpoint"]) {
                            let artist = page_type(&ep["browseEndpoint"]) == "MUSIC_PAGE_TYPE_ARTIST";
                            items.push(MItem::Card(Card { title: title.clone(), subtitle: text(&x["subtitle"]).unwrap_or_default(), thumb: thumb(&x["thumbnail"]), kind: CardKind::Open(b), artist }));
                        } else if let Some(id) = ep["watchEndpoint"]["videoId"].as_str() {
                            let r = runs(&x["subtitle"]);
                            let video = VideoItem { id: id.into(), title: title.clone(), channel: join_artists(&r.artists), channel_id: r.artists.iter().find_map(|a| a.1.clone()), duration: r.duration, music: true, ..Default::default() };
                            items.push(MItem::Song(Song { video, album: r.album, thumb: thumb(&x["thumbnail"]), ..Default::default() }));
                        }
                        // Its songs are the top result's own: an artist's, by name.
                        let by = match items.first() {
                            Some(MItem::Card(c)) if c.artist => Some((c.title.clone(), match &c.kind {
                                CardKind::Open(b) => Some(b.id.clone()),
                                _ => None,
                            })),
                            _ => None,
                        };
                        for mut it in items_of(&x["contents"], None) {
                            if let (MItem::Song(s), Some((name, id))) = (&mut it, &by) {
                                if s.video.channel.is_empty() {
                                    s.video.channel = name.clone();
                                    s.video.channel_id = id.clone();
                                }
                            }
                            items.push(it);
                        }
                        page.sections.push(Section { title: "Top result".into(), more: None, items, text: None });
                    }
                    "musicDescriptionShelfRenderer" => {
                        if let Some(d) = text(&x["description"]) {
                            page.sections.push(Section { title: text(&x["header"]).unwrap_or_default(), text: Some(d), ..Default::default() });
                        }
                    }
                    "appendContinuationItemsAction" => {
                        let items = items_of(&x["continuationItems"], playlist);
                        if !items.is_empty() {
                            page.sections.push(Section { items, ..Default::default() });
                        }
                    }
                    "header" | "musicResponsiveHeaderRenderer" | "musicEditablePlaylistDetailHeaderRenderer" | "menu" | "frameworkUpdates" | "responseContext" | "chipCloudRenderer" => {}
                    _ => collect(x, page, playlist),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect(x, page, playlist)),
        _ => {}
    }
}

/// `--music-dump WHAT`: print a parsed music page (a browse id, "ID|PARAMS",
/// "search:QUERY" or "next:VIDEO"), to check the parser against YouTube.
pub fn dump(what: &str) -> Result<()> {
    let api = Api::new();
    if let Some(v) = what.strip_prefix("credits:") {
        for (role, who) in api.music_credits(v)? {
            println!("{role}: {who}");
        }
        return Ok(());
    }
    if let Some(v) = what.strip_prefix("next:") {
        let n = api.music_next(Some(v), None)?;
        println!("lyrics {:?} related {:?}", n.lyrics, n.related);
        for s in n.queue.iter().take(10) {
            println!("  {} | {} | {:?} | {:?}", s.video.title, s.video.channel, s.album, s.video.duration);
        }
        if let Some(l) = n.lyrics {
            let ly = api.music_lyrics(&l)?;
            println!("lyrics: {:?}… ({})", ly.text.chars().take(80).collect::<String>(), ly.source);
        }
        return Ok(());
    }
    let page = match what.strip_prefix("search:") {
        Some(q) => api.music_search_all(q)?,
        None => match what.split_once('|') {
            Some((id, params)) => api.music_browse(&Browse { id: id.into(), params: Some(params.into()) })?,
            None => api.music_browse(&Browse::new(what))?,
        },
    };
    if let Some(h) = &page.header {
        println!("HEADER {:?} | {:?} | {:?} | play {:?} shuffle {:?} radio {:?} artist {:?} subs {:?} banner {}", h.title, h.subtitle, h.second, h.play, h.shuffle, h.radio, h.artist, h.subscribers, h.banner);
        println!("  desc {:?}", h.description.chars().take(80).collect::<String>());
    }
    for s in &page.sections {
        println!("[{}] more={:?} items={}{}", s.title, s.more.as_ref().map(|b| &b.id), s.items.len(), if s.text.is_some() { " (text)" } else { "" });
        for i in s.items.iter().take(3) {
            match i {
                MItem::Song(x) => println!("  song {} | {} | {:?} | {:?} | {:?} | pl {:?} #{:?} lib {:?}", x.video.title, x.video.channel, x.album.as_ref().map(|a| &a.0), x.video.duration, x.video.views, x.playlist, x.index, x.library.first().map(|l| &l.0)),
                MItem::Card(c) => println!("  card {} | {} | {:?} | artist {}", c.title, c.subtitle, match &c.kind { CardKind::Open(b) => b.id.clone(), CardKind::Play { video, .. } => format!("play {}", video.id) }, c.artist),
                MItem::Nav { label, to, color } => println!("  nav {label} -> {} {:?} {:?}", to.id, to.params, color),
            }
        }
    }
    println!("cont {:?}", page.cont.as_ref().map(|c| c.len()));
    Ok(())
}

/// Where a music.youtube.com URL points.
pub enum Route {
    Home,
    Explore,
    Library,
    Search(String),
    Browse(Browse),
    /// A song or video, with the playlist it plays in.
    Watch(String, Option<String>),
}

/// Map a music.youtube.com URL to a page; None for other hosts.
pub fn route(url: &str) -> Option<Route> {
    let rest = url.trim().trim_start_matches("https://").trim_start_matches("http://").strip_prefix("music.youtube.com")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let q = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| crate::api::urldecode(v)));
    let path = path.trim_end_matches('/');
    Some(match path {
        "" => Route::Home,
        "/explore" => Route::Explore,
        "/library" => Route::Library,
        "/charts" => Route::Browse(Browse::new("FEmusic_charts")),
        "/new_releases" => Route::Browse(Browse::new("FEmusic_new_releases")),
        "/moods_and_genres" => Route::Browse(Browse::new("FEmusic_moods_and_genres")),
        "/search" => Route::Search(q("q").unwrap_or_default()),
        "/watch" => Route::Watch(q("v")?, q("list")),
        "/playlist" => Route::Browse(Browse::new(&format!("VL{}", q("list")?))),
        _ => {
            let id = path.strip_prefix("/browse/").or_else(|| path.strip_prefix("/channel/"))?;
            Route::Browse(Browse::new(id))
        }
    })
}

/// The music.youtube.com address of a browse id.
pub fn address(b: &Browse) -> String {
    match b.id.as_str() {
        "FEmusic_charts" => "music.youtube.com/charts".into(),
        "FEmusic_new_releases" => "music.youtube.com/new_releases".into(),
        "FEmusic_moods_and_genres" => "music.youtube.com/moods_and_genres".into(),
        id if id.starts_with("VL") => format!("music.youtube.com/playlist?list={}", &id[2..]),
        id if id.starts_with("UC") => format!("music.youtube.com/channel/{id}"),
        id => format!("music.youtube.com/browse/{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_music_urls() {
        assert!(matches!(route("https://music.youtube.com/"), Some(Route::Home)));
        assert!(matches!(route("music.youtube.com/explore"), Some(Route::Explore)));
        assert!(matches!(route("https://music.youtube.com/search?q=daft+punk"), Some(Route::Search(q)) if q == "daft punk"));
        assert!(matches!(route("https://music.youtube.com/watch?v=abc&list=PL1"), Some(Route::Watch(v, Some(l))) if v == "abc" && l == "PL1"));
        assert!(matches!(route("https://music.youtube.com/playlist?list=RDCLAK"), Some(Route::Browse(b)) if b.id == "VLRDCLAK"));
        assert!(matches!(route("https://music.youtube.com/browse/MPREb_x"), Some(Route::Browse(b)) if b.id == "MPREb_x"));
        assert!(matches!(route("https://music.youtube.com/channel/UCx"), Some(Route::Browse(b)) if b.id == "UCx"));
        assert!(route("https://www.youtube.com/watch?v=abc").is_none());
        assert_eq!(address(&Browse::new("VLPLx")), "music.youtube.com/playlist?list=PLx");
    }

    #[test]
    fn reads_a_song_row() {
        let row = json!({"musicResponsiveListItemRenderer": {
            "playlistItemData": {"videoId": "v1"},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Robot Rock"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Song"}, {"text": " • "},
                    {"text": "Daft Punk", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCdp", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}},
                    {"text": " • "},
                    {"text": "Human After All", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_h", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}},
                    {"text": " • "}, {"text": "4:47"}
                ]}}}
            ]
        }});
        let items = items_of(&json!([row]), Some("PLx"));
        let MItem::Song(s) = &items[0] else { panic!("not a song") };
        assert_eq!(s.video.id, "v1");
        assert_eq!(s.video.title, "Robot Rock");
        assert_eq!(s.video.channel, "Daft Punk");
        assert_eq!(s.video.channel_id.as_deref(), Some("UCdp"));
        assert_eq!(s.video.duration.as_deref(), Some("4:47"));
        assert_eq!(s.album, Some(("Human After All".into(), "MPREb_h".into())));
        assert_eq!(s.playlist.as_deref(), Some("PLx"));
        assert!(s.video.music);
    }
}
