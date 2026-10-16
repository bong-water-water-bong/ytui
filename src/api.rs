//! Minimal client for YouTube's web JSON API (InnerTube), the same API the
//! youtube.com front end calls. Browsing (search, channels, playlists, watch
//! pages, comments, related videos) goes through here; only stream URLs come
//! from yt-dlp.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
const FALLBACK_VERSION: &str = "2.20261015.01.00";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct VideoItem {
    pub id: String,
    pub title: String,
    pub channel: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub views: Option<String>,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub short: bool,
}

impl VideoItem {
    pub fn thumb(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelItem {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub subs: Option<String>,
    pub thumb: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaylistItem {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub count: Option<String>,
    pub thumb: Option<String>,
    pub first_video: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Video(VideoItem),
    Channel(ChannelItem),
    Playlist(PlaylistItem),
}

#[derive(Clone, Debug)]
pub enum Entry {
    Item(Item),
    Heading(String),
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub entries: Vec<Entry>,
    pub cont: Option<String>,
}

impl Feed {
    pub fn extend(&mut self, more: Feed) {
        let seen: std::collections::HashSet<String> = self.entries.iter().filter_map(entry_key).collect();
        self.entries.extend(more.entries.into_iter().filter(|e| entry_key(e).is_none_or(|k| !seen.contains(&k))));
        self.cont = more.cont;
    }
    pub fn videos(&self) -> Vec<VideoItem> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(Item::Video(v)) => Some(v.clone()),
                _ => None,
            })
            .collect()
    }
}

fn entry_key(e: &Entry) -> Option<String> {
    match e {
        Entry::Item(Item::Video(v)) => Some(format!("v{}", v.id)),
        Entry::Item(Item::Channel(c)) => Some(format!("c{}", c.id)),
        Entry::Item(Item::Playlist(p)) => Some(format!("p{}", p.id)),
        Entry::Heading(_) => None,
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelPage {
    pub name: String,
    pub meta: Vec<String>,
    pub description: String,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    /// (title, params)
    pub tabs: Vec<(String, String)>,
    pub selected: usize,
    pub feed: Feed,
}

#[derive(Clone, Debug, Default)]
pub struct PlaylistPage {
    pub title: String,
    pub meta: Vec<String>,
    pub feed: Feed,
}

#[derive(Clone, Debug, Default)]
pub struct WatchInfo {
    pub title: String,
    pub views: String,
    pub date: String,
    pub likes: String,
    pub channel: String,
    pub channel_id: String,
    pub subs: String,
    pub avatar: Option<String>,
    pub description: String,
    pub comments_token: Option<String>,
    pub related: Feed,
}

#[derive(Clone, Debug, Default)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub time: String,
    pub likes: String,
    pub replies: u32,
    pub replies_token: Option<String>,
    pub creator: bool,
    pub level: u8,
}

#[derive(Clone, Debug, Default)]
pub struct CommentBatch {
    pub count: Option<String>,
    pub comments: Vec<Comment>,
    pub cont: Option<String>,
}

/// What a pasted URL points at.
pub enum Target {
    Video(String),
    Channel(String),
    Playlist(String),
}

pub struct Api {
    agent: ureq::Agent,
    ver: String,
    visitor: String,
}

fn cache_dir() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"));
    base.join("ytui")
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).user_agent(UA).build()
}

impl Api {
    /// Picks up the current web client version (cached for a day).
    pub fn new() -> Api {
        let agent = agent();
        let path = cache_dir().join("innertube.json");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(v) = std::fs::read(&path).map(|b| serde_json::from_slice::<Value>(&b).unwrap_or_default()) {
            if now.saturating_sub(v["ts"].as_u64().unwrap_or(0)) < 86400 {
                if let (Some(ver), Some(vis)) = (v["ver"].as_str(), v["visitor"].as_str()) {
                    return Api { agent, ver: ver.into(), visitor: vis.into() };
                }
            }
        }
        let html = agent
            .get("https://www.youtube.com/?hl=en")
            .set("Accept-Language", "en-US")
            .call()
            .ok()
            .and_then(|r| r.into_string().ok())
            .unwrap_or_default();
        let grab = |key: &str| {
            let pat = format!("\"{key}\":\"");
            html.find(&pat).map(|i| {
                let rest = &html[i + pat.len()..];
                rest[..rest.find('"').unwrap_or(0)].to_string()
            })
        };
        let ver = grab("INNERTUBE_CLIENT_VERSION").unwrap_or_else(|| FALLBACK_VERSION.into());
        let visitor = grab("VISITOR_DATA").unwrap_or_default();
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(&path, json!({"ver": ver, "visitor": visitor, "ts": now}).to_string());
        Api { agent, ver, visitor }
    }

    fn post(&self, ep: &str, mut body: Value) -> Result<Value> {
        body["context"] = json!({"client": {"clientName": "WEB", "clientVersion": self.ver, "hl": "en", "gl": "US", "visitorData": self.visitor}});
        let r = self
            .agent
            .post(&format!("https://www.youtube.com/youtubei/v1/{ep}?prettyPrint=false"))
            .set("Content-Type", "application/json")
            .set("X-YouTube-Client-Name", "1")
            .set("X-YouTube-Client-Version", &self.ver)
            .set("Origin", "https://www.youtube.com")
            .send_string(&body.to_string());
        match r {
            Ok(r) => Ok(serde_json::from_str(&r.into_string()?)?),
            Err(ureq::Error::Status(code, _)) => bail!("YouTube returned HTTP {code}"),
            Err(e) => bail!("network: {e}"),
        }
    }

    pub fn search(&self, q: &str, sp: Option<&str>) -> Result<Feed> {
        let mut body = json!({"query": q});
        if let Some(sp) = sp {
            body["params"] = json!(sp);
        }
        Ok(feed_from(&self.post("search", body)?))
    }

    pub fn search_more(&self, token: &str) -> Result<Feed> {
        Ok(feed_from(&self.post("search", json!({"continuation": token}))?))
    }

    pub fn browse_more(&self, token: &str) -> Result<Feed> {
        Ok(feed_from(&self.post("browse", json!({"continuation": token}))?))
    }

    pub fn channel(&self, id: &str, params: Option<&str>) -> Result<ChannelPage> {
        let mut body = json!({"browseId": id});
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        let v = self.post("browse", body)?;
        let mut page = ChannelPage::default();
        let hdr = find(&v["header"], "pageHeaderViewModel");
        if let Some(h) = hdr {
            page.name = text(&h["title"]).unwrap_or_default();
            page.meta = meta_rows(&h["metadata"]).into_iter().flatten().collect();
            page.description = text(&h["description"]["descriptionPreviewViewModel"]["description"]).unwrap_or_default();
            page.avatar = first_url(&h["image"]);
            page.banner = last_url(&h["banner"]);
        } else if let Some(h) = find(&v["header"], "c4TabbedHeaderRenderer") {
            page.name = text(&h["title"]).unwrap_or_default();
            page.meta = ["channelHandleText", "subscriberCountText", "videosCountText"]
                .iter()
                .filter_map(|k| text(&h[*k]))
                .collect();
            page.avatar = last_url(&h["avatar"]);
            page.banner = last_url(&h["banner"]);
        }
        if page.name.is_empty() {
            page.name = text(&v["metadata"]["channelMetadataRenderer"]["title"]).unwrap_or_default();
        }
        if page.description.is_empty() {
            page.description = v["metadata"]["channelMetadataRenderer"]["description"].as_str().unwrap_or("").into();
        }
        let tabs = v["contents"]["twoColumnBrowseResultsRenderer"]["tabs"].as_array().cloned().unwrap_or_default();
        for t in tabs.iter().filter_map(|t| t.get("tabRenderer")) {
            let title = t["title"].as_str().unwrap_or("").to_string();
            let params = t["endpoint"]["browseEndpoint"]["params"].as_str().unwrap_or("").to_string();
            if !matches!(title.as_str(), "Home" | "Videos" | "Shorts" | "Live" | "Playlists" | "Podcasts" | "Releases") {
                continue;
            }
            if t["selected"].as_bool() == Some(true) {
                page.selected = page.tabs.len();
                page.feed = feed_from(&t["content"]);
            }
            page.tabs.push((title, params));
        }
        Ok(page)
    }

    pub fn playlist(&self, id: &str) -> Result<PlaylistPage> {
        let v = self.post("browse", json!({"browseId": format!("VL{id}")}))?;
        let mut page = PlaylistPage::default();
        if let Some(h) = find(&v["header"], "pageHeaderViewModel") {
            page.title = text(&h["title"]).unwrap_or_default();
            page.meta = meta_rows(&h["metadata"]).into_iter().flatten().collect();
        }
        if page.title.is_empty() {
            page.title = v["metadata"]["playlistMetadataRenderer"]["title"].as_str().unwrap_or("Playlist").into();
        }
        page.feed = feed_from(&v["contents"]);
        if page.feed.entries.is_empty() {
            if let Some(alert) = find(&v["alerts"], "text") {
                bail!("{}", text(alert).unwrap_or_else(|| "playlist unavailable".into()));
            }
        }
        Ok(page)
    }

    pub fn watch(&self, id: &str) -> Result<WatchInfo> {
        let v = self.post("next", json!({"videoId": id}))?;
        let res = &v["contents"]["twoColumnWatchNextResults"];
        let mut w = WatchInfo::default();
        if let Some(p) = find(&res["results"], "videoPrimaryInfoRenderer") {
            w.title = text(&p["title"]).unwrap_or_default();
            w.views = text(&p["viewCount"]["videoViewCountRenderer"]["viewCount"]).unwrap_or_default();
            w.date = text(&p["dateText"]).unwrap_or_default();
            if let Some(l) = find(p, "likeButtonViewModel") {
                w.likes = find(l, "buttonViewModel").and_then(|b| b["title"].as_str()).unwrap_or("").into();
            }
        }
        if let Some(s) = find(&res["results"], "videoSecondaryInfoRenderer") {
            let o = &s["owner"]["videoOwnerRenderer"];
            // Collaborations carry attributedTitle and list channels in a dialog.
            w.channel = text(&o["title"]).filter(|t| !t.is_empty()).or_else(|| text(&o["attributedTitle"])).unwrap_or_default();
            w.channel_id = o["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str().map(String::from).or_else(|| browse_id(o)).unwrap_or_default();
            w.subs = text(&o["subscriberCountText"]).unwrap_or_else(|| if o.get("avatarStack").is_some() { "Collaboration".into() } else { String::new() });
            w.avatar = last_url(&o["thumbnail"]);
            w.description = text(&s["attributedDescription"]).unwrap_or_default();
        }
        let mut secs = Vec::new();
        find_all(&res["results"], "itemSectionRenderer", &mut secs);
        for s in secs {
            if s["sectionIdentifier"].as_str() == Some("comment-item-section") {
                w.comments_token = find(s, "continuationCommand").and_then(|c| c["token"].as_str()).map(String::from);
            }
        }
        w.related = feed_from(&res["secondaryResults"]);
        if w.title.is_empty() {
            bail!("video unavailable");
        }
        Ok(w)
    }

    pub fn comments(&self, token: &str) -> Result<CommentBatch> {
        let v = self.post("next", json!({"continuation": token}))?;
        let mut payloads = std::collections::HashMap::new();
        let mut ps = Vec::new();
        find_all(&v["frameworkUpdates"], "commentEntityPayload", &mut ps);
        for p in ps {
            if let Some(k) = p["key"].as_str() {
                payloads.insert(k.to_string(), p);
            }
        }
        let mut out = CommentBatch::default();
        let mut hdr = Vec::new();
        find_all(&v, "commentsHeaderRenderer", &mut hdr);
        out.count = hdr.first().and_then(|h| text(&h["countText"]));
        fn walk(v: &Value, pay: &std::collections::HashMap<String, &Value>, out: &mut CommentBatch, level: u8) {
            match v {
                Value::Object(m) => {
                    for (k, x) in m {
                        match k.as_str() {
                            "commentThreadRenderer" => {
                                let key = x["commentViewModel"]["commentViewModel"]["commentKey"].as_str().unwrap_or("");
                                if let Some(mut c) = pay.get(key).map(|p| comment(p, level)) {
                                    c.replies_token = find(&x["replies"], "continuationCommand")
                                        .and_then(|t| t["token"].as_str())
                                        .map(String::from);
                                    out.comments.push(c);
                                }
                            }
                            "commentViewModel" => {
                                if let Some(key) = x["commentKey"].as_str() {
                                    if let Some(p) = pay.get(key) {
                                        out.comments.push(comment(p, level));
                                    }
                                } else {
                                    walk(x, pay, out, level);
                                }
                            }
                            "continuationItemRenderer" => {
                                out.cont = find(x, "continuationCommand").and_then(|t| t["token"].as_str()).map(String::from);
                            }
                            "frameworkUpdates" | "header" | "sortFilterSubMenuRenderer" | "commentsHeaderRenderer" => {}
                            _ => walk(x, pay, out, level),
                        }
                    }
                }
                Value::Array(a) => a.iter().for_each(|x| walk(x, pay, out, level)),
                _ => {}
            }
        }
        walk(&v["onResponseReceivedEndpoints"], &payloads, &mut out, 0);
        Ok(out)
    }

    pub fn resolve_url(&self, url: &str) -> Result<Target> {
        if let Some(id) = video_id_from_url(url) {
            return Ok(Target::Video(id));
        }
        if let Some(list) = query_param(url, "list") {
            return Ok(Target::Playlist(list));
        }
        let v = self.post("navigation/resolve_url", json!({"url": url}))?;
        let ep = &v["endpoint"];
        if let Some(id) = ep["watchEndpoint"]["videoId"].as_str() {
            return Ok(Target::Video(id.into()));
        }
        if let Some(id) = ep["browseEndpoint"]["browseId"].as_str() {
            if let Some(pl) = id.strip_prefix("VL") {
                return Ok(Target::Playlist(pl.into()));
            }
            return Ok(Target::Channel(id.into()));
        }
        Err(anyhow!("couldn't open that URL"))
    }

    pub fn suggest(&self, q: &str) -> Vec<String> {
        let url = format!(
            "https://suggestqueries-clients6.youtube.com/complete/search?client=youtube&ds=yt&q={}",
            urlencode(q)
        );
        let Ok(body) = self.agent.get(&url).call().and_then(|r| r.into_string().map_err(Into::into)) else {
            return Vec::new();
        };
        let (Some(a), Some(b)) = (body.find('['), body.rfind(']')) else { return Vec::new() };
        let Ok(v) = serde_json::from_str::<Value>(&body[a..=b]) else { return Vec::new() };
        v[1].as_array()
            .map(|xs| xs.iter().filter_map(|x| x[0].as_str().map(String::from)).take(8).collect())
            .unwrap_or_default()
    }

    /// Latest uploads of a channel via its public RSS feed.
    pub fn rss(&self, channel_id: &str) -> Result<Vec<(u64, VideoItem)>> {
        let url = format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}");
        let body = self.agent.get(&url).call()?.into_string()?;
        let tag = |s: &str, t: &str| -> Option<String> {
            let open = format!("<{t}>");
            let i = s.find(&open)? + open.len();
            Some(xml_unescape(&s[i..i + s[i..].find(&format!("</{t}>"))?]))
        };
        let author = body.split("<entry>").next().and_then(|h| tag(h, "name")).unwrap_or_default();
        let now = now_secs();
        let mut out = Vec::new();
        for e in body.split("<entry>").skip(1) {
            let Some(id) = tag(e, "yt:videoId") else { continue };
            let ts = tag(e, "published").map(|p| parse_iso(&p)).unwrap_or(0);
            let views = e
                .find("views=\"")
                .map(|i| &e[i + 7..])
                .and_then(|r| r[..r.find('"')?].parse::<u64>().ok())
                .map(|n| format!("{} views", short_count(n)));
            out.push((
                ts,
                VideoItem {
                    id,
                    title: tag(e, "title").unwrap_or_default(),
                    channel: author.clone(),
                    channel_id: Some(channel_id.into()),
                    views,
                    published: Some(ago(now.saturating_sub(ts))),
                    short: e.contains("/shorts/"),
                    ..Default::default()
                },
            ));
        }
        Ok(out)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn parse_iso(s: &str) -> u64 {
    // 2026-10-14T16:00:07+00:00
    let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d, hh, mm, ss) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + hh * 3600 + mm * 60 + ss).max(0) as u64
}

pub fn ago(secs: u64) -> String {
    let (n, u) = match secs {
        0..=3599 => (secs / 60, "minute"),
        3600..=86399 => (secs / 3600, "hour"),
        86400..=604799 => (secs / 86400, "day"),
        604800..=2591999 => (secs / 604800, "week"),
        2592000..=31535999 => (secs / 2592000, "month"),
        _ => (secs / 31536000, "year"),
    };
    let n = n.max(1);
    format!("{n} {u}{} ago", if n == 1 { "" } else { "s" })
}

pub fn short_count(v: u64) -> String {
    match v {
        0..=999 => format!("{v}"),
        1_000..=999_999 => format!("{:.1}K", v as f64 / 1e3).replace(".0K", "K"),
        1_000_000..=999_999_999 => format!("{:.1}M", v as f64 / 1e6).replace(".0M", "M"),
        _ => format!("{:.1}B", v as f64 / 1e9).replace(".0B", "B"),
    }
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key && !v.is_empty()).then(|| v.split('#').next().unwrap_or(v).to_string())
    })
}

pub fn video_id_from_url(url: &str) -> Option<String> {
    if let Some(v) = query_param(url, "v") {
        return Some(v);
    }
    for pat in ["youtu.be/", "/shorts/", "/live/", "/embed/"] {
        if let Some(i) = url.find(pat) {
            let id: String = url[i + pat.len()..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if id.len() == 11 {
                return Some(id);
            }
        }
    }
    None
}

pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    !s.contains(' ')
        && (s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("www.")
            || s.starts_with("youtube.com")
            || s.starts_with("m.youtube.com")
            || s.starts_with("youtu.be")
            || s.starts_with('@'))
}

// ---------------------------------------------------------------- parsing

/// Plain text from simpleText / runs / content shapes.
pub fn text(v: &Value) -> Option<String> {
    if let Some(s) = v["simpleText"].as_str() {
        return Some(s.to_string());
    }
    if let Some(runs) = v["runs"].as_array() {
        return Some(runs.iter().filter_map(|r| r["text"].as_str()).collect());
    }
    if let Some(s) = v["content"].as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = v["dynamicTextViewModel"]["text"]["content"].as_str() {
        return Some(s.to_string());
    }
    v.as_str().map(String::from)
}

/// First object under `key` anywhere inside `v` (depth-first).
pub fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(key) {
                return Some(x);
            }
            m.values().find_map(|x| find(x, key))
        }
        Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    find_all(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find_all(x, key, out)),
        _ => {}
    }
}

fn fix_url(u: &str) -> String {
    if let Some(r) = u.strip_prefix("//") {
        format!("https://{r}")
    } else {
        u.to_string()
    }
}

fn urls(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut lists = Vec::new();
    find_all(v, "sources", &mut lists);
    find_all(v, "thumbnails", &mut lists);
    for l in lists {
        if let Some(a) = l.as_array() {
            out.extend(a.iter().filter_map(|x| x["url"].as_str()).map(fix_url));
        }
    }
    out
}

fn first_url(v: &Value) -> Option<String> {
    urls(v).into_iter().next()
}

fn last_url(v: &Value) -> Option<String> {
    urls(v).into_iter().last()
}

fn meta_rows(v: &Value) -> Vec<Vec<String>> {
    find(v, "metadataRows")
        .and_then(|r| r.as_array())
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r["metadataParts"]
                        .as_array()
                        .map(|ps| ps.iter().filter_map(|p| text(&p["text"])).filter(|s| !s.trim().is_empty()).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn browse_id(v: &Value) -> Option<String> {
    let mut eps = Vec::new();
    find_all(v, "browseEndpoint", &mut eps);
    eps.iter().filter_map(|e| e["browseId"].as_str()).find(|id| id.starts_with("UC")).map(String::from)
}

fn feed_from(v: &Value) -> Feed {
    let mut feed = Feed::default();
    collect(v, &mut feed);
    feed
}

fn collect(v: &Value, feed: &mut Feed) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let item = match k.as_str() {
                    "videoRenderer" | "gridVideoRenderer" | "compactVideoRenderer" | "playlistVideoRenderer" | "playlistPanelVideoRenderer" => {
                        video_renderer(x).map(Item::Video)
                    }
                    "lockupViewModel" => lockup(x),
                    "shortsLockupViewModel" => short_lockup(x).map(Item::Video),
                    "reelItemRenderer" => x["videoId"].as_str().map(|id| {
                        Item::Video(VideoItem {
                            id: id.into(),
                            title: text(&x["headline"]).unwrap_or_default(),
                            views: text(&x["viewCountText"]),
                            short: true,
                            ..Default::default()
                        })
                    }),
                    "channelRenderer" | "gridChannelRenderer" => channel_renderer(x).map(Item::Channel),
                    "playlistRenderer" | "gridPlaylistRenderer" | "radioRenderer" | "compactRadioRenderer" => playlist_renderer(x).map(Item::Playlist),
                    "shelfRenderer" | "reelShelfRenderer" | "richShelfRenderer" => {
                        if let Some(t) = text(&x["title"]).filter(|t| !t.is_empty()) {
                            feed.entries.push(Entry::Heading(t));
                        }
                        collect(x, feed);
                        None
                    }
                    "gridShelfViewModel" => {
                        if let Some(t) = find(&x["header"], "title").and_then(text) {
                            feed.entries.push(Entry::Heading(t));
                        }
                        collect(&x["contents"], feed);
                        None
                    }
                    "continuationItemRenderer" => {
                        if let Some(t) = find(x, "continuationCommand").and_then(|c| c["token"].as_str()) {
                            feed.cont = Some(t.into());
                        }
                        None
                    }
                    "adSlotRenderer" | "promotedSparklesWebRenderer" | "searchPyvRenderer" | "inFeedAdLayoutRenderer" | "frameworkUpdates"
                    | "engagementPanels" | "topbar" | "microformat" | "responseContext" | "menu" | "menuRenderer" | "sheetViewModel"
                    | "header" | "chipCloudRenderer" | "feedFilterChipBarRenderer" | "chipBarViewModel" | "messageRenderer"
                    | "backgroundPromoRenderer" | "movieRenderer" | "horizontalCardListRenderer" | "postRenderer" | "sharedPostRenderer"
                    | "channelVideoPlayerRenderer" | "relatedChipCloudRenderer" => None,
                    _ => {
                        collect(x, feed);
                        None
                    }
                };
                if let Some(i) = item {
                    feed.entries.push(Entry::Item(i));
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect(x, feed)),
        _ => {}
    }
}

fn video_renderer(x: &Value) -> Option<VideoItem> {
    let id = x["videoId"].as_str()?.to_string();
    let byline = ["longBylineText", "ownerText", "shortBylineText"].iter().map(|k| &x[*k]).find(|b| !b.is_null()).cloned().unwrap_or_default();
    let mut duration = text(&x["lengthText"]);
    if duration.is_none() {
        duration = find(&x["thumbnailOverlays"], "thumbnailOverlayTimeStatusRenderer").and_then(|t| text(&t["text"]));
    }
    let views = text(&x["viewCountText"]).or_else(|| text(&x["shortViewCountText"]));
    let live = x["badges"].to_string().contains("LIVE") || views.as_deref().is_some_and(|v| v.contains("watching"));
    Some(VideoItem {
        id,
        title: text(&x["title"]).unwrap_or_default(),
        channel: text(&byline).unwrap_or_default(),
        channel_id: browse_id(&byline),
        duration: if live { None } else { duration },
        views,
        published: text(&x["publishedTimeText"]),
        live,
        short: false,
    })
}

fn lockup(x: &Value) -> Option<Item> {
    let id = x["contentId"].as_str()?.to_string();
    let md = &x["metadata"]["lockupMetadataViewModel"];
    let title = text(&md["title"]).unwrap_or_default();
    let rows = meta_rows(&md["metadata"]);
    let badge = find(&x["contentImage"], "thumbnailBadgeViewModel").and_then(|b| b["text"].as_str()).map(String::from);
    match x["contentType"].as_str()? {
        "LOCKUP_CONTENT_TYPE_VIDEO" => {
            let live = badge.as_deref() == Some("LIVE");
            let r1 = rows.get(1).cloned().unwrap_or_default();
            Some(Item::Video(VideoItem {
                id,
                title,
                channel: rows.first().and_then(|r| r.first()).cloned().unwrap_or_default(),
                channel_id: browse_id(md),
                duration: if live { None } else { badge },
                views: r1.first().cloned().map(|v| if v.ends_with("views") || v.contains("watching") { v } else { format!("{v} views") }),
                published: r1.get(1).cloned(),
                live,
                short: false,
            }))
        }
        "LOCKUP_CONTENT_TYPE_PLAYLIST" | "LOCKUP_CONTENT_TYPE_ALBUM" | "LOCKUP_CONTENT_TYPE_PODCAST" => {
            let first = find(&x["rendererContext"], "watchEndpoint").and_then(|w| w["videoId"].as_str()).map(String::from);
            Some(Item::Playlist(PlaylistItem {
                id,
                title,
                channel: rows.first().and_then(|r| r.first()).cloned(),
                count: badge,
                thumb: first_url(&x["contentImage"]),
                first_video: first,
            }))
        }
        _ => None,
    }
}

fn short_lockup(x: &Value) -> Option<VideoItem> {
    let id = find(&x["onTap"], "reelWatchEndpoint").and_then(|r| r["videoId"].as_str())?.to_string();
    Some(VideoItem {
        id,
        title: text(&x["overlayMetadata"]["primaryText"]).unwrap_or_default(),
        views: text(&x["overlayMetadata"]["secondaryText"]),
        short: true,
        ..Default::default()
    })
}

fn channel_renderer(x: &Value) -> Option<ChannelItem> {
    let id = x["channelId"].as_str()?.to_string();
    let a = text(&x["subscriberCountText"]);
    let b = text(&x["videoCountText"]);
    // Search results put the handle in subscriberCountText nowadays.
    let (handle, subs) = match &a {
        Some(h) if h.starts_with('@') => (a.clone(), b),
        _ => (None, a.or(b)),
    };
    Some(ChannelItem { id, name: text(&x["title"]).unwrap_or_default(), handle, subs, thumb: last_url(&x["thumbnail"]) })
}

fn playlist_renderer(x: &Value) -> Option<PlaylistItem> {
    let id = x["playlistId"].as_str()?.to_string();
    Some(PlaylistItem {
        id,
        title: text(&x["title"]).unwrap_or_default(),
        channel: text(&x["shortBylineText"]).or_else(|| text(&x["longBylineText"])),
        count: text(&x["videoCountText"]).or_else(|| x["videoCount"].as_str().map(|c| format!("{c} videos"))),
        thumb: first_url(&x["thumbnails"]).or_else(|| first_url(&x["thumbnail"])),
        first_video: find(&x["navigationEndpoint"], "watchEndpoint").and_then(|w| w["videoId"].as_str()).map(String::from),
    })
}

fn comment(p: &Value, level: u8) -> Comment {
    let pr = &p["properties"];
    let tb = &p["toolbar"];
    Comment {
        author: p["author"]["displayName"].as_str().unwrap_or("").into(),
        text: pr["content"]["content"].as_str().unwrap_or("").into(),
        time: pr["publishedTime"].as_str().unwrap_or("").into(),
        likes: tb["likeCountNotliked"].as_str().unwrap_or("").trim().into(),
        replies: tb["replyCount"].as_str().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        replies_token: None,
        creator: p["author"]["isCreator"].as_bool() == Some(true),
        level: pr["replyLevel"].as_u64().map(|l| l as u8).unwrap_or(level),
    }
}
