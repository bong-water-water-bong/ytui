//! Signing in for playback: YouTube sometimes refuses to serve a connection
//! until it can "confirm you're not a bot", and the fix is the cookies of a
//! browser that's signed in to YouTube. Browsers lock and encrypt their own
//! cookie stores, so instead the user exports them with a cookie extension
//! (which copies them to the clipboard or saves cookies.txt) and ytui picks
//! them up from the clipboard or a paste, converts them to the cookies.txt
//! format yt-dlp reads, and saves them privately.

use anyhow::{bail, Result};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Cookie {
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: i64,
    pub name: String,
    pub value: String,
}

/// Where the saved cookies live (yt-dlp's --cookies file).
pub fn path() -> PathBuf {
    crate::yt::cookies_path()
}

/// Cookies from a clipboard / paste: a cookie extension's JSON export
/// (Cookie-Editor, EditThisCookie, ...) or a Netscape cookies.txt.
pub fn parse(text: &str) -> Option<Vec<Cookie>> {
    let t = text.trim();
    let cookies = if t.starts_with('[') || t.starts_with('{') { parse_json(t)? } else { parse_netscape(t) };
    (!cookies.is_empty()).then_some(cookies)
}

fn parse_json(t: &str) -> Option<Vec<Cookie>> {
    let v: Value = serde_json::from_str(t).ok()?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("cookies").and_then(|c| c.as_array()).cloned()?,
        _ => return None,
    };
    let mut out = Vec::new();
    for c in arr {
        let (Some(domain), Some(name), Some(value)) = (c["domain"].as_str(), c["name"].as_str(), c["value"].as_str()) else { continue };
        let host_only = c["hostOnly"].as_bool().unwrap_or(false);
        let domain = if !host_only && !domain.starts_with('.') { format!(".{domain}") } else { domain.to_string() };
        let expires = c["expirationDate"].as_f64().or_else(|| c["expires"].as_f64()).unwrap_or(0.0).max(0.0) as i64;
        out.push(Cookie {
            domain,
            path: c["path"].as_str().unwrap_or("/").to_string(),
            secure: c["secure"].as_bool().unwrap_or(false),
            http_only: c["httpOnly"].as_bool().unwrap_or(false),
            expires,
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Some(out)
}

fn parse_netscape(t: &str) -> Vec<Cookie> {
    t.lines()
        .filter_map(|l| {
            let (http_only, l) = match l.strip_prefix("#HttpOnly_") {
                Some(rest) => (true, rest),
                None if l.starts_with('#') => return None,
                None => (false, l),
            };
            let f: Vec<&str> = l.trim_end_matches('\r').split('\t').collect();
            (f.len() == 7).then(|| Cookie {
                domain: f[0].to_string(),
                path: f[2].to_string(),
                secure: f[3] == "TRUE",
                http_only,
                expires: f[4].parse().unwrap_or(0),
                name: f[5].to_string(),
                value: f[6].to_string(),
            })
        })
        .collect()
}

/// The export is from a browser signed in to YouTube (not just a visit).
pub fn signed_in(cookies: &[Cookie]) -> bool {
    const AUTH: [&str; 4] = ["SAPISID", "__Secure-3PSID", "__Secure-1PSID", "LOGIN_INFO"];
    cookies.iter().any(|c| c.domain.trim_start_matches('.').ends_with("youtube.com") && AUTH.contains(&c.name.as_str()))
}

/// Keep only YouTube / Google cookies and write them as cookies.txt,
/// readable only by this user. Returns how many were saved.
pub fn save(cookies: &[Cookie]) -> Result<usize> {
    let keep: Vec<&Cookie> = cookies
        .iter()
        .filter(|c| {
            let d = c.domain.trim_start_matches('.');
            d.ends_with("youtube.com") || d.ends_with("google.com")
        })
        .collect();
    if keep.is_empty() {
        bail!("no YouTube cookies in that export");
    }
    let mut s = String::from("# Netscape HTTP Cookie File\n# Saved by ytui (sign in for playback)\n\n");
    for c in &keep {
        let sub = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
        let dom = if c.http_only { format!("#HttpOnly_{}", c.domain) } else { c.domain.clone() };
        s.push_str(&format!("{dom}\t{sub}\t{}\t{}\t{}\t{}\t{}\n", c.path, if c.secure { "TRUE" } else { "FALSE" }, c.expires, c.name, c.value));
    }
    crate::sys::write_private(&path(), s.as_bytes())?;
    crate::yt::reload_cookies();
    Ok(keep.len())
}

/// Forget the saved cookies.
pub fn remove() {
    let _ = std::fs::remove_file(path());
    crate::yt::reload_cookies();
}

/// The saved sign-in's cookies (empty when signed out).
pub fn load() -> Vec<Cookie> {
    std::fs::read_to_string(path()).map(|t| parse_netscape(&t)).unwrap_or_default()
}

/// (cookie count, saved how long ago) for the saved file, if any.
pub fn status() -> Option<(usize, std::time::Duration)> {
    let p = path();
    let text = std::fs::read_to_string(&p).ok()?;
    let n = parse_netscape(&text).len();
    let age = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().unwrap_or_default();
    Some((n, age))
}

/// Ask YouTube whether the saved cookies get past the bot check.
pub fn verify() -> Result<()> {
    crate::yt::check_access()
}

// ------------------------------------------------------------- clipboard

/// A number that changes whenever the clipboard does (0 if unknown), so the
/// clipboard is only read when there's something new.
pub fn clipboard_seq() -> u32 {
    #[cfg(windows)]
    unsafe {
        return windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber();
    }
    #[allow(unreachable_code)]
    0
}

/// The clipboard's text, where we can read it (Windows).
pub fn clipboard_text() -> Option<String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::DataExchange::*;
        use windows_sys::Win32::System::Memory::*;
        const CF_UNICODETEXT: u32 = 13;
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h as _) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h as _);
            }
        }
        CloseClipboard();
        return out;
    }
    #[allow(unreachable_code)]
    None
}

/// Empty the clipboard (it held sign-in cookies).
pub fn clear_clipboard() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::DataExchange::*;
        if OpenClipboard(std::ptr::null_mut()) != 0 {
            EmptyClipboard();
            CloseClipboard();
        }
    }
}

/// `ytui --signin`: a Google sign-in window, like signing in on a website;
/// the cookie-extension flow if there's no usable browser.
pub fn signin_cli() -> Result<()> {
    println!("Sign in to YouTube\n");
    println!("A browser window is opening on Google's sign-in page. Sign in there as you");
    println!("would on any website; this finishes by itself (Ctrl-C to cancel).\n");
    match crate::browser_login::sign_in_blocking(std::time::Duration::from_secs(600)) {
        Ok(c) => {
            let n = save(&c)?;
            println!("Signed in ({n} cookies saved to {}).", path().display());
            print!("Checking with YouTube… ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            match verify() {
                Ok(()) => println!("done. Videos will play."),
                Err(e) => println!("not accepted: {e}"),
            }
            return Ok(());
        }
        Err(e) => println!("Couldn't use a browser window ({e}).\nUsing your cookie extension instead.\n"),
    }
    signin_clipboard()
}

fn signin_clipboard() -> Result<()> {
    println!("1. In your browser, go to youtube.com and make sure you're signed in.");
    println!("2. Click your cookie extension (e.g. Cookie-Editor or Get cookies.txt LOCALLY)");
    println!("   and press Export or Copy.\n");
    crate::sys::open_url("https://www.youtube.com");
    let start = clipboard_seq();
    let can_watch = cfg!(windows);
    if can_watch {
        println!("Waiting for the cookies on the clipboard… (Ctrl-C to cancel)");
    } else {
        println!("Paste the exported cookies here, then press Ctrl-D:");
    }
    let text = if can_watch {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if std::time::Instant::now() > deadline {
                bail!("gave up waiting for cookies");
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
            if clipboard_seq() == start {
                continue;
            }
            let Some(t) = clipboard_text() else { continue };
            match parse(&t) {
                Some(c) if signed_in(&c) => break t,
                Some(_) => println!("That export isn't from a signed-in YouTube tab; sign in on youtube.com and export again."),
                None => {}
            }
        }
    } else {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
        t
    };
    let Some(cookies) = parse(&text) else { bail!("that doesn't look like a cookie export") };
    if !signed_in(&cookies) {
        bail!("those cookies aren't from a signed-in YouTube session");
    }
    let n = save(&cookies)?;
    clear_clipboard();
    println!("\nSaved {n} cookies to {} (clipboard cleared).", path().display());
    print!("Checking with YouTube… ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match verify() {
        Ok(()) => println!("signed in. Videos will play."),
        Err(e) => println!("not accepted: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookie_editor_json() {
        let j = r#"[{"domain":".youtube.com","hostOnly":false,"httpOnly":true,"name":"SAPISID","path":"/","secure":true,"expirationDate":1800000000.5,"value":"x"},
                   {"domain":"www.youtube.com","hostOnly":true,"name":"PREF","path":"/","secure":false,"value":"y"}]"#;
        let c = parse(j).unwrap();
        assert_eq!(c.len(), 2);
        assert!(signed_in(&c));
        assert_eq!(c[0].expires, 1800000000);
        assert!(c[0].http_only && c[0].secure);
        assert_eq!(c[1].domain, "www.youtube.com");
    }

    #[test]
    fn reads_netscape() {
        let t = "# Netscape HTTP Cookie File\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\tv\n.youtube.com\tTRUE\t/\tFALSE\t5\tPREF\tf=1\n";
        let c = parse(t).unwrap();
        assert_eq!(c.len(), 2);
        assert!(c[0].http_only);
        assert!(signed_in(&c));
    }

    #[test]
    fn rejects_other_text_and_signed_out_exports() {
        assert!(parse("hello world").is_none());
        let j = r#"[{"domain":".youtube.com","name":"PREF","value":"y"}]"#;
        assert!(!signed_in(&parse(j).unwrap()));
    }
}
