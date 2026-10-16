//! "Sign in" the way websites do it: open a browser window on Google's
//! sign-in page, let the user sign in there, and take the resulting YouTube
//! session for yt-dlp.
//!
//! The window is a separate Chrome / Edge / Brave / Chromium instance with
//! its own profile under ytui's data directory (never the user's everyday
//! profile, whose cookies are locked and encrypted). It runs as a completely
//! ordinary browser: Google refuses to sign in a browser that has a
//! remote-debugging port open ("try again a different way"). ytui only
//! watches the window title; once it shows YouTube (sign-in done) ytui
//! closes the window, then starts that profile again headless, with a local
//! debugging port, just long enough to read its cookies. That second run
//! never talks to Google's sign-in, so the check doesn't apply.

use crate::cookies::Cookie;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

const SIGN_IN: &str = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3D%252F";

/// A Chromium-family browser to sign in with.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YTUI_BROWSER") {
        return Some(PathBuf::from(p));
    }
    let mut c: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(b) = std::env::var_os(base) {
                let b = PathBuf::from(b);
                c.push(b.join("Google/Chrome/Application/chrome.exe"));
                c.push(b.join("Microsoft/Edge/Application/msedge.exe"));
                c.push(b.join("BraveSoftware/Brave-Browser/Application/brave.exe"));
                c.push(b.join("Chromium/Application/chrome.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        for app in ["Google Chrome.app/Contents/MacOS/Google Chrome", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Chromium.app/Contents/MacOS/Chromium", "Brave Browser.app/Contents/MacOS/Brave Browser"] {
            c.push(PathBuf::from("/Applications").join(app));
        }
    } else {
        for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
            if let Some(p) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|p| p.is_file())) {
                c.push(p);
            }
        }
    }
    c.into_iter().find(|p| p.is_file())
}

fn profile_dir() -> PathBuf {
    crate::auth::data_dir().join("browser")
}

fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Out of ytui's kill-on-exit job: Chrome manages its own processes.
        cmd.creation_flags(0x0100_0000);
    }
    cmd.spawn()
}

pub enum Poll {
    /// Still signing in.
    Waiting,
    /// Signed in: the YouTube / Google cookies.
    Done(Vec<Cookie>),
    /// The window closed without a YouTube sign-in.
    NotSignedIn,
    Failed(String),
}

/// A sign-in in progress; the work happens on its own thread.
pub struct Login {
    rx: Receiver<Result<Option<Vec<Cookie>>>>,
    cancel: Arc<AtomicBool>,
    pub started: Instant,
}

impl Login {
    /// Open the sign-in window.
    pub fn start() -> Result<Login> {
        let exe = find_browser().ok_or_else(|| anyhow!("no Chrome, Edge, Brave or Chromium found (set YTUI_BROWSER)"))?;
        let dir = profile_dir();
        std::fs::create_dir_all(&dir)?;
        // An earlier sign-in window still open would keep the profile (a new
        // window just joins it, and the cookies can't be read afterwards).
        close_leftovers();
        let child = spawn(Command::new(&exe).arg(format!("--user-data-dir={}", dir.display())).args(["--no-first-run", "--no-default-browser-check", "--new-window", SIGN_IN]))
            .map_err(|e| anyhow!("couldn't start {}: {e}", exe.display()))?;
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::Builder::new()
            .name("ytui-signin".into())
            .spawn(move || {
                let _ = tx.send(run(exe, dir, child, c));
            })?;
        Ok(Login { rx, cancel, started: Instant::now() })
    }

    pub fn poll(&mut self) -> Poll {
        match self.rx.try_recv() {
            Ok(Ok(Some(c))) => Poll::Done(c),
            Ok(Ok(None)) => Poll::NotSignedIn,
            Ok(Err(e)) => Poll::Failed(e.to_string()),
            Err(TryRecvError::Empty) => Poll::Waiting,
            Err(TryRecvError::Disconnected) => Poll::Failed("sign-in stopped".into()),
        }
    }

    /// Give up: close the window.
    pub fn close(self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Wait for the sign-in to finish (or the window to close), then read the
/// profile's cookies.
fn run(exe: PathBuf, dir: PathBuf, mut child: Child, cancel: Arc<AtomicBool>) -> Result<Option<Vec<Cookie>>> {
    let pid = child.id();
    // The window may show "YouTube" before anything happens (Google titles
    // its YouTube sign-in that way, or skips to youtube.com), so only treat
    // YouTube as "done" after a Google sign-in page has been seen.
    // Otherwise the user closes the window when they're finished.
    let mut saw_sign_in = false;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            close_windows(pid);
            wait_exit(&mut child, Duration::from_secs(5));
            return Ok(None);
        }
        let titles = window_titles(pid);
        if titles.iter().any(|t| t.contains("Sign in") || t.contains("Google Account") || t.starts_with("Google")) {
            saw_sign_in = true;
        }
        if saw_sign_in && titles.iter().any(|t| t.starts_with("YouTube")) {
            // Let the YouTube page finish setting its cookies first.
            std::thread::sleep(Duration::from_millis(1500));
            close_windows(pid);
            wait_exit(&mut child, Duration::from_secs(10));
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let cookies = read_cookies(&exe, &dir)?;
    Ok(crate::cookies::signed_in(&cookies).then_some(cookies))
}

/// Stop any browser still running on ytui's sign-in profile.
fn close_leftovers() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe' OR Name='msedge.exe' OR Name='brave.exe'\" | Where-Object { $_.CommandLine -match 'ytui[\\/]browser' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }",
            ])
            .creation_flags(0x0800_0000) // no console window
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn wait_exit(child: &mut Child, t: Duration) {
    let deadline = Instant::now() + t;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
}

/// Titles of the visible top-level windows of a process (Windows only;
/// elsewhere the user closes the window when done).
fn window_titles(pid: u32) -> Vec<String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        struct Ctx {
            pid: u32,
            out: Vec<String>,
        }
        unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
            let ctx = &mut *(l as *mut Ctx);
            let mut p = 0u32;
            GetWindowThreadProcessId(h, &mut p);
            if p == ctx.pid && IsWindowVisible(h) != 0 {
                let mut b = [0u16; 512];
                let n = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
                if n > 0 {
                    ctx.out.push(String::from_utf16_lossy(&b[..n as usize]));
                }
            }
            1
        }
        let mut ctx = Ctx { pid, out: Vec::new() };
        EnumWindows(Some(each), &mut ctx as *mut Ctx as LPARAM);
        return ctx.out;
    }
    #[allow(unreachable_code)]
    {
        let _ = pid;
        Vec::new()
    }
}

/// Ask the process's windows to close (like clicking X), so the browser
/// shuts down cleanly and writes its cookies out.
fn close_windows(pid: u32) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
            let mut p = 0u32;
            GetWindowThreadProcessId(h, &mut p);
            if p == l as u32 && IsWindowVisible(h) != 0 {
                PostMessageW(h, WM_CLOSE, 0, 0);
            }
            1
        }
        EnumWindows(Some(each), pid as LPARAM);
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Start the profile headless with a local debugging port and read all its
/// cookies (decrypted by the browser itself), then shut it down.
fn read_cookies(exe: &Path, dir: &Path) -> Result<Vec<Cookie>> {
    // The visible browser may still be letting go of the profile.
    let lock = dir.join("lockfile");
    let deadline = Instant::now() + Duration::from_secs(10);
    while lock.exists() && Instant::now() < deadline {
        let _ = std::fs::remove_file(&lock); // fails while it's really held
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(dir.join("DevToolsActivePort"));
    let mut child = spawn(
        Command::new(exe)
            .arg(format!("--user-data-dir={}", dir.display()))
            .args(["--headless=new", "--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check", "about:blank"]),
    )?;
    let result = (|| -> Result<Vec<Cookie>> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let info = loop {
            if let Ok(s) = std::fs::read_to_string(dir.join("DevToolsActivePort")) {
                if s.lines().count() >= 2 {
                    break s;
                }
            }
            if Instant::now() > deadline {
                bail!("the browser didn't start for reading the sign-in");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut lines = info.lines();
        let (port, path) = (lines.next().unwrap_or("").trim(), lines.next().unwrap_or("").trim());
        let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}{path}"))?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        }
        let r = call(&mut ws, 1, "Storage.getCookies")?;
        let _ = call(&mut ws, 2, "Browser.close");
        Ok(r["cookies"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        Some(Cookie {
                            domain: c["domain"].as_str()?.to_string(),
                            path: c["path"].as_str().unwrap_or("/").to_string(),
                            secure: c["secure"].as_bool().unwrap_or(false),
                            http_only: c["httpOnly"].as_bool().unwrap_or(false),
                            expires: c["expires"].as_f64().unwrap_or(0.0).max(0.0) as i64,
                            name: c["name"].as_str()?.to_string(),
                            value: c["value"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    })();
    wait_exit(&mut child, Duration::from_secs(5));
    result
}

fn call(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, id: u64, method: &str) -> Result<Value> {
    ws.send(Message::text(json!({ "id": id, "method": method }).to_string()))?;
    loop {
        let m = ws.read()?;
        let Ok(v) = serde_json::from_str::<Value>(m.to_text().unwrap_or("")) else { continue };
        if v["id"].as_u64() == Some(id) {
            if let Some(e) = v.get("error") {
                bail!("{method}: {e}");
            }
            return Ok(v["result"].clone());
        }
    }
}

/// Sign in, blocking (for `ytui --signin`). Returns the cookies.
pub fn sign_in_blocking(timeout: Duration) -> Result<Vec<Cookie>> {
    let mut l = Login::start()?;
    loop {
        if l.started.elapsed() > timeout {
            l.close();
            bail!("gave up waiting for the sign-in");
        }
        match l.poll() {
            Poll::Waiting => std::thread::sleep(Duration::from_millis(300)),
            Poll::Done(c) => return Ok(c),
            Poll::NotSignedIn => bail!("the window closed before you were signed in to YouTube"),
            Poll::Failed(e) => bail!("{e}"),
        }
    }
}

/// `ytui --signin-probe`: read the sign-in profile's cookies headless.
pub fn probe() -> Result<()> {
    let exe = find_browser().ok_or_else(|| anyhow!("no browser"))?;
    println!("browser: {}", exe.display());
    let t = Instant::now();
    let c = read_cookies(&exe, &profile_dir())?;
    let yt = c.iter().filter(|c| c.domain.contains("youtube.com")).count();
    println!("read {} cookies ({yt} for youtube.com) in {:.1}s; signed in to YouTube: {}", c.len(), t.elapsed().as_secs_f64(), crate::cookies::signed_in(&c));
    Ok(())
}

/// `ytui --signin-titles`: open the sign-in window and log its titles
/// for a few seconds (diagnostics), then close it.
pub fn log_titles() -> Result<()> {
    let exe = find_browser().ok_or_else(|| anyhow!("no browser"))?;
    let dir = profile_dir();
    close_leftovers();
    let mut child = spawn(Command::new(&exe).arg(format!("--user-data-dir={}", dir.display())).args(["--no-first-run", "--no-default-browser-check", "--new-window", SIGN_IN]))?;
    let t = Instant::now();
    let mut last = Vec::new();
    while t.elapsed() < Duration::from_secs(8) {
        let titles = window_titles(child.id());
        if titles != last {
            println!("{:5.1}s {:?}", t.elapsed().as_secs_f64(), titles);
            last = titles;
        }
        if child.try_wait().ok().flatten().is_some() {
            println!("browser exited");
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    close_windows(child.id());
    wait_exit(&mut child, Duration::from_secs(5));
    Ok(())
}

/// `ytui --signin-hold`: start a real sign-in and check the window is still
/// open (not closed on its own) after a few seconds, then cancel it.
pub fn hold_test() -> Result<()> {
    let mut l = Login::start()?;
    for s in 1..=8 {
        std::thread::sleep(Duration::from_secs(1));
        match l.poll() {
            Poll::Waiting => println!("{s}s: window still open, waiting for you"),
            Poll::Done(_) => bail!("{s}s: finished on its own"),
            Poll::NotSignedIn => bail!("{s}s: window closed on its own"),
            Poll::Failed(e) => bail!("{s}s: failed: {e}"),
        }
    }
    l.close();
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}
