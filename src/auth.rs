//! Signing in with Google (OAuth 2.0 for installed apps, loopback redirect
//! + PKCE) and the YouTube Data API v3 calls that need it: rating videos and
//! posting comments / replies.
//!
//! The OAuth client (a "Desktop app" client from the user's own Google Cloud
//! project) lives in ~/.config/ytui/client.json; tokens in
//! ~/.local/share/ytui/oauth.json. Both are written mode 0600.

use crate::api::Comment;
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

const SCOPE: &str = "https://www.googleapis.com/auth/youtube.force-ssl";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/youtube/v3";

#[derive(Clone, Serialize, Deserialize)]
struct ClientCfg {
    client_id: String,
    client_secret: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    expires_at: u64,
    channel: String,
    channel_id: String,
}

pub struct Auth {
    client: ClientCfg,
    tokens: Mutex<Tokens>,
    agent: ureq::Agent,
    /// video id -> liveChatId
    chat_ids: Mutex<std::collections::HashMap<String, String>>,
}

/// One of your YouTube subscriptions.
pub struct RemoteSub {
    pub channel_id: String,
    pub title: String,
    /// Subscription resource id (needed to unsubscribe).
    pub sub_id: String,
}

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("ytui")
}

fn data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"))
        .join("ytui")
}

fn client_path() -> PathBuf {
    config_dir().join("client.json")
}

fn token_path() -> PathBuf {
    data_dir().join("oauth.json")
}

fn write_private(path: &PathBuf, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn now() -> u64 {
    crate::api::now_secs()
}

fn random_token(n: usize) -> String {
    let mut b = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // Not expected on Linux; still unpredictable enough for a state value.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        b = Sha256::digest(format!("{t}{}", std::process::id())).to_vec();
    }
    URL_SAFE_NO_PAD.encode(b)
}

/// Turn an HTTP error from Google into a readable message.
fn api_err(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            let msg = v["error"]["message"]
                .as_str()
                .or(v["error_description"].as_str())
                .or(v["error"].as_str())
                .unwrap_or("request failed")
                .to_string();
            let reason = v["error"]["errors"][0]["reason"].as_str().unwrap_or("");
            match reason {
                "quotaExceeded" => anyhow!("YouTube API daily quota used up (resets at midnight Pacific)"),
                "commentsDisabled" => anyhow!("comments are turned off for this video"),
                "subscriptionDuplicate" => anyhow!("already subscribed"),
                "subscriptionForbidden" => anyhow!("YouTube refused the subscription (too many recent subscriptions or the channel can't be subscribed to)"),
                "subscriberNotFound" | "subscriptionNotFound" => anyhow!("not subscribed on YouTube"),
                "liveChatDisabled" => anyhow!("live chat is turned off for this stream"),
                "liveChatEnded" => anyhow!("this live chat has ended"),
                "rateLimitExceeded" => anyhow!("sending too fast (slow mode?); wait a moment"),
                "forbidden" if code == 403 => anyhow!("not allowed: {}", strip_tags(&msg)),
                _ => anyhow!("HTTP {code}: {}", strip_tags(&msg)),
            }
        }
        e => anyhow!("network: {e}"),
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out
}

impl Auth {
    pub fn load() -> Option<Auth> {
        let client: ClientCfg = serde_json::from_slice(&std::fs::read(client_path()).ok()?).ok()?;
        let tokens: Tokens = serde_json::from_slice(&std::fs::read(token_path()).ok()?).ok()?;
        Some(Auth { client, tokens: Mutex::new(tokens), agent: crate::api::agent(), chat_ids: Mutex::default() })
    }

    pub fn channel(&self) -> String {
        self.tokens.lock().unwrap().channel.clone()
    }

    /// A valid access token, refreshing it when it is about to expire.
    fn access(&self) -> Result<String> {
        let mut t = self.tokens.lock().unwrap();
        if t.expires_at > now() + 60 {
            return Ok(t.access.clone());
        }
        let r = self
            .agent
            .post(TOKEN_URL)
            .send_form(&[
                ("client_id", &self.client.client_id),
                ("client_secret", &self.client.client_secret),
                ("refresh_token", &t.refresh),
                ("grant_type", "refresh_token"),
            ])
            .map_err(|e| match api_err(e) {
                e if e.to_string().contains("invalid_grant") || e.to_string().contains("expired") => {
                    anyhow!("sign-in expired; run `ytui --login` again")
                }
                e => e,
            })?;
        let v: Value = serde_json::from_str(&r.into_string()?)?;
        t.access = v["access_token"].as_str().ok_or_else(|| anyhow!("no access token in refresh response"))?.into();
        t.expires_at = now() + v["expires_in"].as_u64().unwrap_or(3600);
        if let Some(rt) = v["refresh_token"].as_str() {
            t.refresh = rt.into();
        }
        let _ = write_private(&token_path(), &serde_json::to_vec(&*t)?);
        Ok(t.access.clone())
    }

    fn post_json(&self, url: &str, body: Value) -> Result<Value> {
        let tok = self.access()?;
        let r = self
            .agent
            .post(url)
            .set("Authorization", &format!("Bearer {tok}"))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(api_err)?;
        let s = r.into_string()?;
        Ok(if s.trim().is_empty() { Value::Null } else { serde_json::from_str(&s)? })
    }

    /// "like", "dislike" or "none".
    pub fn rating(&self, video: &str) -> Result<String> {
        let tok = self.access()?;
        let r = self
            .agent
            .get(&format!("{API}/videos/getRating?id={video}"))
            .set("Authorization", &format!("Bearer {tok}"))
            .call()
            .map_err(api_err)?;
        let v: Value = serde_json::from_str(&r.into_string()?)?;
        Ok(v["items"][0]["rating"].as_str().unwrap_or("none").to_string())
    }

    pub fn rate(&self, video: &str, rating: &str) -> Result<()> {
        let tok = self.access()?;
        self.agent
            .post(&format!("{API}/videos/rate?id={video}&rating={rating}"))
            .set("Authorization", &format!("Bearer {tok}"))
            .send_string("")
            .map_err(api_err)?;
        Ok(())
    }

    pub fn comment(&self, video: &str, text: &str) -> Result<Comment> {
        let body = json!({"snippet": {"videoId": video, "topLevelComment": {"snippet": {"textOriginal": text}}}});
        let v = self.post_json(&format!("{API}/commentThreads?part=snippet"), body)?;
        Ok(posted(&v["snippet"]["topLevelComment"], 0))
    }

    pub fn reply(&self, parent: &str, text: &str) -> Result<Comment> {
        let body = json!({"snippet": {"parentId": parent, "textOriginal": text}});
        let v = self.post_json(&format!("{API}/comments?part=snippet"), body)?;
        Ok(posted(&v, 1))
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let tok = self.access()?;
        let r = self.agent.get(url).set("Authorization", &format!("Bearer {tok}")).call().map_err(api_err)?;
        Ok(serde_json::from_str(&r.into_string()?)?)
    }

    /// All of your subscriptions (1 quota unit per 50).
    pub fn subscriptions(&self) -> Result<Vec<RemoteSub>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..40 {
            let mut url = format!("{API}/subscriptions?part=snippet&mine=true&maxResults=50&order=alphabetical");
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={p}"));
            }
            let v = self.get_json(&url)?;
            for it in v["items"].as_array().into_iter().flatten() {
                let sn = &it["snippet"];
                if let Some(ch) = sn["resourceId"]["channelId"].as_str() {
                    out.push(RemoteSub {
                        channel_id: ch.into(),
                        title: sn["title"].as_str().unwrap_or("").into(),
                        sub_id: it["id"].as_str().unwrap_or("").into(),
                    });
                }
            }
            page = v["nextPageToken"].as_str().map(String::from);
            if page.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Subscribe; returns the new subscription id.
    pub fn subscribe(&self, channel: &str) -> Result<String> {
        let body = json!({"snippet": {"resourceId": {"kind": "youtube#channel", "channelId": channel}}});
        let v = self.post_json(&format!("{API}/subscriptions?part=snippet"), body)?;
        Ok(v["id"].as_str().unwrap_or("").into())
    }

    pub fn unsubscribe(&self, sub_id: &str) -> Result<()> {
        let tok = self.access()?;
        self.agent
            .delete(&format!("{API}/subscriptions?id={sub_id}"))
            .set("Authorization", &format!("Bearer {tok}"))
            .call()
            .map_err(api_err)?;
        Ok(())
    }

    fn live_chat_id(&self, video: &str) -> Result<String> {
        if let Some(id) = self.chat_ids.lock().unwrap().get(video) {
            return Ok(id.clone());
        }
        let v = self.get_json(&format!("{API}/videos?part=liveStreamingDetails&id={video}"))?;
        let id = v["items"][0]["liveStreamingDetails"]["activeLiveChatId"]
            .as_str()
            .ok_or_else(|| anyhow!("this video has no active live chat"))?
            .to_string();
        self.chat_ids.lock().unwrap().insert(video.into(), id.clone());
        Ok(id)
    }

    /// Post a message to a live stream's chat.
    pub fn send_chat(&self, video: &str, text: &str) -> Result<()> {
        let chat = self.live_chat_id(video)?;
        let body = json!({"snippet": {"liveChatId": chat, "type": "textMessageEvent", "textMessageDetails": {"messageText": text}}});
        self.post_json(&format!("{API}/liveChat/messages?part=snippet"), body)?;
        Ok(())
    }

    /// Revoke the grant and forget the tokens.
    pub fn sign_out(&self) {
        let t = self.tokens.lock().unwrap().clone();
        let _ = self.agent.post("https://oauth2.googleapis.com/revoke").send_form(&[("token", &t.refresh)]);
        let _ = std::fs::remove_file(token_path());
    }
}

fn posted(c: &Value, level: u8) -> Comment {
    let s = &c["snippet"];
    Comment {
        id: c["id"].as_str().unwrap_or("").into(),
        author: s["authorDisplayName"].as_str().unwrap_or("you").into(),
        text: s["textOriginal"].as_str().or(s["textDisplay"].as_str()).unwrap_or("").into(),
        time: "just now".into(),
        level,
        ..Default::default()
    }
}

// ------------------------------------------------------------------ login

/// Read a downloaded client_secret_*.json (or our own saved format).
fn read_client(path: &str) -> Result<ClientCfg> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).with_context(|| format!("reading {path}"))?)?;
    let o = v.get("installed").or(v.get("web")).unwrap_or(&v);
    let id = o["client_id"].as_str().ok_or_else(|| anyhow!("{path}: no client_id"))?;
    let secret = o["client_secret"].as_str().unwrap_or("");
    if v.get("web").is_some() {
        eprintln!("note: this is a \"Web application\" client; a \"Desktop app\" client is what ytui expects.");
    }
    Ok(ClientCfg { client_id: id.into(), client_secret: secret.into() })
}

fn query_param(s: &str, key: &str) -> Option<String> {
    let q = s.split_once('?').map(|x| x.1).unwrap_or(s);
    q.split(['&', ' ']).find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into()
}

enum Got {
    Redirect(String),
    Pasted(String),
}

/// Interactive sign-in from the command line.
pub fn login(client_file: Option<&str>) -> Result<()> {
    let client = match client_file {
        Some(f) => {
            let c = read_client(f)?;
            write_private(&client_path(), &serde_json::to_vec_pretty(&c)?)?;
            c
        }
        None => serde_json::from_slice(&std::fs::read(client_path()).map_err(|_| {
            anyhow!("no OAuth client configured yet.\nRun: ytui --login --client /path/to/client_secret.json\n(see `ytui --help-login` for how to create one)")
        })?)?,
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(16);
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&access_type=offline&prompt=consent&state={state}",
        crate::api::urlencode(&client.client_id),
        crate::api::urlencode(&redirect),
        crate::api::urlencode(SCOPE),
    );

    println!("Sign in to YouTube\n");
    println!("1. Open this link in a browser and sign in with your Google account:\n\n   {url}\n");
    println!("2. When Google says it's done, this finishes automatically.");
    println!("   If the browser is on a different machine (e.g. you're on SSH), the last page");
    println!("   won't load (it points at 127.0.0.1:{port}). That's expected: copy that page's");
    println!("   full address from the browser's address bar and paste it here.\n");
    if std::env::var("SSH_CONNECTION").is_err() && std::env::var("DISPLAY").is_ok() {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    print!("Waiting… paste the address here (or press Ctrl-C to cancel): ");
    let _ = std::io::stdout().flush();

    let (tx, rx) = std::sync::mpsc::channel::<Got>();
    let txl = tx.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut line = String::new();
            if BufReader::new(&s).read_line(&mut line).is_err() {
                continue;
            }
            let target = line.split_whitespace().nth(1).unwrap_or("").to_string();
            if !target.contains("code=") && !target.contains("error=") {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                continue;
            }
            let body = "<!doctype html><meta charset=utf-8><title>ytui</title><body style='font:16px sans-serif;background:#111;color:#eee;padding:3em'><h2>Got it</h2><p>Go back to the terminal: ytui is finishing the sign-in there. You can close this tab.</p>";
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = txl.send(Got::Redirect(target));
            return;
        }
    });
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            let l = line.trim().to_string();
            if !l.is_empty() {
                let _ = tx.send(Got::Pasted(l));
                return;
            }
        }
    });
    let got = rx.recv()?;
    let raw = match &got {
        Got::Redirect(t) | Got::Pasted(t) => t.clone(),
    };
    if matches!(got, Got::Redirect(_)) {
        println!();
    }
    if let Some(e) = query_param(&raw, "error") {
        bail!("Google returned an error: {e}");
    }
    // A pasted bare code (no URL) is accepted too.
    let code = query_param(&raw, "code").unwrap_or_else(|| raw.clone());
    if let Some(st) = query_param(&raw, "state") {
        if st != state {
            bail!("sign-in response didn't match this request (state mismatch); try again");
        }
    }

    let r = crate::api::agent()
        .post(TOKEN_URL)
        .send_form(&[
            ("code", code.as_str()),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("redirect_uri", &redirect),
            ("grant_type", "authorization_code"),
            ("code_verifier", &verifier),
        ])
        .map_err(api_err)?;
    let v: Value = serde_json::from_str(&r.into_string()?)?;
    let access = v["access_token"].as_str().ok_or_else(|| anyhow!("no access token returned"))?.to_string();
    let refresh = v["refresh_token"].as_str().ok_or_else(|| anyhow!("no refresh token returned; remove ytui at https://myaccount.google.com/permissions and sign in again"))?.to_string();
    let mut tokens = Tokens { access, refresh, expires_at: now() + v["expires_in"].as_u64().unwrap_or(3600), channel: String::new(), channel_id: String::new() };

    // Which channel did we sign in as?
    let me = crate::api::agent()
        .get(&format!("{API}/channels?part=snippet&mine=true"))
        .set("Authorization", &format!("Bearer {}", tokens.access))
        .call()
        .map_err(api_err)
        .and_then(|r| Ok(serde_json::from_str::<Value>(&r.into_string()?)?));
    match me {
        Ok(me) => {
            tokens.channel = me["items"][0]["snippet"]["title"].as_str().unwrap_or("").into();
            tokens.channel_id = me["items"][0]["id"].as_str().unwrap_or("").into();
        }
        Err(e) => eprintln!("warning: signed in, but couldn't read your channel: {e}"),
    }
    write_private(&token_path(), &serde_json::to_vec_pretty(&tokens)?)?;
    if tokens.channel.is_empty() {
        println!("Signed in. (This Google account has no YouTube channel yet; create one on youtube.com to comment.)");
    } else {
        println!("Signed in as {}.", tokens.channel);
    }
    Ok(())
}

pub fn logout() -> Result<()> {
    match Auth::load() {
        Some(a) => {
            a.sign_out();
            println!("Signed out.");
        }
        None => println!("Not signed in."),
    }
    Ok(())
}

pub const SETUP_HELP: &str = "\
Signing in uses Google's official OAuth with your own (free) Google Cloud client.
One-time setup, about 5 minutes:

 1. Go to https://console.cloud.google.com/ and create a project (any name).
 2. APIs & Services -> Library: search \"YouTube Data API v3\" and click Enable.
 3. Google Auth Platform -> Branding (OAuth consent screen): set an app name and
    your email. Audience: User type \"External\"; under Test users add the Google
    account you'll sign in with.
 4. Google Auth Platform -> Clients -> Create client -> Application type
    \"Desktop app\" -> Create -> Download JSON.
 5. Run:  ytui --login --client ~/Downloads/client_secret_XXXX.json

Notes
 - While the app is in \"Testing\", Google expires the sign-in after 7 days; run
   `ytui --login` again then. Publishing the app (Audience -> Publish) removes the
   limit; Google then shows an \"unverified app\" warning you can click through.
 - The free API quota is 10,000 units/day: a like or a comment costs 50, so about
   200 actions a day.
 - `ytui --logout` revokes ytui's access. You can also remove it at
   https://myaccount.google.com/permissions
";
