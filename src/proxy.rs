// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

//! Local HTTP relay for googlevideo stream URLs.
//!
//! YouTube throttles open-ended requests (`Range: bytes=N-`, which is all
//! ffmpeg ever sends) to a stall after ~800 KB, while bounded ranges arrive
//! at full speed. So ffmpeg reads from us instead, with ordinary requests
//! and seeks, and we fetch upstream in bounded chunks. Each chunk is read
//! into memory at network speed and then handed over at ffmpeg's pace, so
//! no upstream connection sits idle while playback is paused.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CHUNK: u64 = 8 << 20;

struct Relay {
    port: u16,
    /// Upstream URL and total size, indexed by the path we hand out.
    streams: Mutex<Vec<(String, u64)>>,
    agent: ureq::Agent,
}

static RELAY: OnceLock<Option<Relay>> = OnceLock::new();

fn relay() -> Option<&'static Relay> {
    RELAY
        .get_or_init(|| {
            let l = TcpListener::bind("127.0.0.1:0").ok()?;
            let port = l.local_addr().ok()?.port();
            std::thread::spawn(move || {
                for c in l.incoming().flatten() {
                    std::thread::spawn(move || {
                        let _ = serve(c);
                    });
                }
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(15))
                .build();
            Some(Relay { port, streams: Mutex::new(Vec::new()), agent })
        })
        .as_ref()
}

/// The URL ffmpeg should open for `url`: a local relay address for
/// fixed-size googlevideo files, otherwise `url` unchanged (live / HLS
/// streams are fetched in segments already).
pub fn wrap(url: &str) -> String {
    let Some(len) = url.contains("googlevideo.com/videoplayback").then(|| param(url, "clen")).flatten().and_then(|v| v.parse().ok()) else {
        return url.to_string();
    };
    let Some(r) = relay() else { return url.to_string() };
    let mut s = r.streams.lock().unwrap();
    s.push((url.to_string(), len));
    format!("http://127.0.0.1:{}/{}", r.port, s.len() - 1)
}

fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = &url[url.find('?')? + 1..];
    q.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn serve(c: TcpStream) -> std::io::Result<()> {
    let r = relay().unwrap();
    let mut rd = BufReader::new(c.try_clone()?);
    let mut line = String::new();
    rd.read_line(&mut line)?;
    let id: Option<usize> = line.split_whitespace().nth(1).and_then(|p| p.trim_start_matches('/').parse().ok());
    let mut range: Option<(u64, Option<u64>)> = None;
    loop {
        let mut h = String::new();
        if rd.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range = v.trim().strip_prefix("bytes=").and_then(|v| {
                    let (a, b) = v.split_once('-')?;
                    Some((a.parse().ok()?, b.trim().parse().ok()))
                });
            }
        }
    }
    let mut out = c;
    let Some((url, len)) = id.and_then(|i| r.streams.lock().unwrap().get(i).cloned()) else {
        return out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    };
    let (start, end) = match range {
        Some((a, b)) => (a, b.unwrap_or(len - 1).min(len - 1)),
        None => (0, len - 1),
    };
    if start >= len || start > end {
        let h = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return out.write_all(h.as_bytes());
    }
    let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
    let h = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        end - start + 1
    );
    out.write_all(h.as_bytes())?;
    let mut pos = start;
    let mut buf = Vec::with_capacity(CHUNK as usize);
    let mut fails = 0;
    while pos <= end {
        let to = (pos + CHUNK - 1).min(end);
        buf.clear();
        let got = r.agent.get(&url).set("Range", &format!("bytes={pos}-{to}")).call().map_err(std::io::Error::other).and_then(|resp| {
            resp.into_reader().take(to - pos + 1).read_to_end(&mut buf)
        });
        if got.is_err() || buf.is_empty() {
            fails += 1;
            if fails > 3 {
                // Ends the response short; ffmpeg's -reconnect picks it up.
                return Err(got.err().unwrap_or_else(|| std::io::Error::other("empty upstream chunk")));
            }
            std::thread::sleep(Duration::from_millis(300 * fails));
        } else {
            fails = 0;
        }
        out.write_all(&buf)?;
        pos += buf.len() as u64;
    }
    Ok(())
}
