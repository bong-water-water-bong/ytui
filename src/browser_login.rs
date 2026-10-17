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
//!
//! Firefox works too, with your everyday profile: it keeps its cookies
//! unencrypted in cookies.sqlite. If you're already signed in to YouTube
//! there, ytui takes that sign-in at once; otherwise it opens Google's
//! sign-in page as a tab in your Firefox and finishes as soon as YouTube's
//! sign-in cookies show up. Without an everyday profile, it uses a separate
//! Firefox profile of its own, the same way.

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

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// A browser to sign in with: Chrome / Edge / Brave / Chromium, else Firefox.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YTUI_BROWSER") {
        let p = PathBuf::from(p);
        // A bare name like `firefox` is looked up on PATH.
        if p.components().count() == 1 && !p.is_file() {
            return on_path(&p.to_string_lossy()).or(Some(p));
        }
        return Some(p);
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
        for base in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(b) = std::env::var_os(base) {
                c.push(PathBuf::from(b).join("Mozilla Firefox/firefox.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        for app in ["Google Chrome.app/Contents/MacOS/Google Chrome", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Chromium.app/Contents/MacOS/Chromium", "Brave Browser.app/Contents/MacOS/Brave Browser", "Firefox.app/Contents/MacOS/firefox"] {
            c.push(PathBuf::from("/Applications").join(app));
        }
    } else {
        for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser", "firefox", "firefox-esr"] {
            c.extend(on_path(name));
        }
    }
    c.into_iter().find(|p| p.is_file())
}

fn profile_dir() -> PathBuf {
    crate::auth::data_dir().join("browser")
}

fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // In a tmux started outside the desktop, the desktop's Wayland display
    // is missing and the browser can't open a window: point it at the
    // session's socket.
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if let Some(n) = (0..4).map(|i| format!("wayland-{i}")).find(|n| Path::new(&dir).join(n).exists()) {
                cmd.env("WAYLAND_DISPLAY", n);
            }
        }
    }
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
        let exe = find_browser().ok_or_else(|| anyhow!("no Chrome, Edge, Brave, Chromium or Firefox found (set YTUI_BROWSER)"))?;
        if is_firefox(&exe) {
            if let Some(dir) = firefox::everyday_profile(&exe) {
                return Ok(Login::watch(move |c| firefox::run_everyday(exe, dir, c)));
            }
            let dir = firefox_profile(&exe);
            let child = firefox::start(&exe, &dir).map_err(|e| anyhow!("couldn't start {}: {e}", exe.display()))?;
            return Ok(Login::watch(move |c| firefox::run(dir, child, c)));
        }
        let dir = profile_dir();
        std::fs::create_dir_all(&dir)?;
        // An earlier sign-in window still open would keep the profile (a new
        // window just joins it, and the cookies can't be read afterwards).
        close_leftovers();
        let child = spawn(Command::new(&exe).arg(format!("--user-data-dir={}", dir.display())).args(["--no-first-run", "--no-default-browser-check", "--new-window", SIGN_IN]))
            .map_err(|e| anyhow!("couldn't start {}: {e}", exe.display()))?;
        Ok(Login::watch(move |c| run(exe, dir, child, c)))
    }

    /// Run `work` on its own thread; its result is what `poll` reports.
    fn watch(work: impl FnOnce(Arc<AtomicBool>) -> Result<Option<Vec<Cookie>>> + Send + 'static) -> Login {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(c));
        });
        Login { rx, cancel, started: Instant::now() }
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

/// A browser that quits within seconds never showed its window.
const NO_WINDOW: &str = "the browser closed straight away (it couldn't open a window: is ytui running inside your desktop session?)";

/// Wait for the sign-in to finish (or the window to close), then read the
/// profile's cookies.
fn run(exe: PathBuf, dir: PathBuf, mut child: Child, cancel: Arc<AtomicBool>) -> Result<Option<Vec<Cookie>>> {
    let pid = child.id();
    let started = Instant::now();
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
    if !saw_sign_in && started.elapsed() < Duration::from_secs(5) {
        bail!(NO_WINDOW);
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
    let c = if is_firefox(&exe) {
        let dir = firefox::everyday_profile(&exe).unwrap_or_else(|| firefox_profile(&exe));
        println!("profile: {}", dir.display());
        firefox::read_cookies(&dir)?
    } else {
        read_cookies(&exe, &profile_dir())?
    };
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

fn is_firefox(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("firefox"))
}

/// Ubuntu's Firefox is a snap, which can't open folders hidden in the home
/// directory (like ~/.local/share), only its own ~/snap/firefox/common.
fn is_snap(exe: &Path) -> bool {
    if std::fs::canonicalize(exe).is_ok_and(|p| p.starts_with("/snap")) {
        return true;
    }
    // /usr/bin/firefox on Ubuntu is a small script that runs the snap.
    std::fs::metadata(exe).is_ok_and(|m| m.len() < 64 * 1024) && std::fs::read(exe).is_ok_and(|b| String::from_utf8_lossy(&b).contains("/snap/bin/firefox"))
}

fn snap_profile() -> PathBuf {
    crate::sys::home().join("snap/firefox/common/ytui-signin")
}

fn firefox_profile(exe: &Path) -> PathBuf {
    if is_snap(exe) {
        snap_profile()
    } else {
        crate::auth::data_dir().join("firefox")
    }
}

/// The snap's sign-in profile, outside ytui's own folders (for `ytui --uninstall`).
pub fn firefox_profiles() -> Vec<PathBuf> {
    vec![snap_profile()]
}

mod firefox {
    use super::*;

    /// Prefs for the sign-in profile: no welcome tour, default-browser
    /// question or telemetry notice in the way of Google's page.
    const PREFS: &str = r#"user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("browser.tabs.warnOnClose", false);
"#;

    pub fn start(exe: &Path, dir: &Path) -> std::io::Result<Child> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("user.js"), PREFS)?;
        // A sign-in left in the profile would finish at once, and you may
        // want a different account: every sign-in starts with no cookies.
        for f in ["cookies.sqlite", "cookies.sqlite-wal", "cookies.sqlite-shm"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        let mut cmd = Command::new(exe);
        cmd.arg("--profile").arg(dir).args(["--no-remote", "--new-instance"]);
        // Without this the Windows launcher process exits at once and the
        // window looks closed.
        if cfg!(windows) {
            cmd.arg("-wait-for-browser");
        }
        spawn(cmd.arg(SIGN_IN))
    }

    /// Watch the profile's cookies until YouTube's sign-in cookies appear
    /// (or the window closes), then close Firefox.
    pub fn run(dir: PathBuf, mut child: Child, cancel: Arc<AtomicBool>) -> Result<Option<Vec<Cookie>>> {
        let started = Instant::now();
        let mut seen = None;
        loop {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                stop(&mut child);
                return Ok(None);
            }
            if let Some(c) = read_cookies(&dir).ok().filter(|c| crate::cookies::signed_in(c)) {
                // Let YouTube finish setting its cookies first.
                std::thread::sleep(Duration::from_millis(1500));
                seen = read_cookies(&dir).ok().filter(|c| crate::cookies::signed_in(c)).or(Some(c));
                stop(&mut child);
                break;
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        if seen.is_none() && started.elapsed() < Duration::from_secs(5) {
            bail!(NO_WINDOW);
        }
        // Firefox writes everything out when it closes; read once more.
        let cookies = read_cookies(&dir).ok().filter(|c| crate::cookies::signed_in(c)).or(seen);
        Ok(cookies)
    }

    /// Where Firefox keeps its profiles (profiles.ini).
    fn roots(exe: &Path) -> Vec<PathBuf> {
        let home = crate::sys::home();
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Mozilla/Firefox")).into_iter().collect()
        } else if cfg!(target_os = "macos") {
            vec![home.join("Library/Application Support/Firefox")]
        } else if is_snap(exe) {
            vec![home.join("snap/firefox/common/.mozilla/firefox")]
        } else {
            vec![home.join(".mozilla/firefox"), home.join(".config/mozilla/firefox")]
        }
    }

    /// Your everyday Firefox profile: the one profiles.ini says this install
    /// uses, else the one marked default.
    pub fn everyday_profile(exe: &Path) -> Option<PathBuf> {
        for root in roots(exe) {
            let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else { continue };
            let (mut install, mut default) = (None, None);
            let (mut section, mut path, mut relative, mut is_default) = (String::new(), None::<String>, true, false);
            let mut end = |section: &str, path: Option<String>, relative: bool, is_default: bool| {
                if section.starts_with("Profile") && is_default {
                    if let Some(p) = path {
                        default = Some(if relative { root.join(p) } else { PathBuf::from(p) });
                    }
                }
            };
            for line in ini.lines().map(str::trim) {
                if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                    end(&section, path.take(), relative, is_default);
                    (section, relative, is_default) = (name.to_string(), true, false);
                } else if let Some((k, v)) = line.split_once('=') {
                    match (k, section.starts_with("Install")) {
                        ("Default", true) if install.is_none() => install = Some(root.join(v)),
                        ("Default", false) => is_default = v == "1",
                        ("Path", _) => path = Some(v.to_string()),
                        ("IsRelative", _) => relative = v == "1",
                        _ => {}
                    }
                }
            }
            end(&section, path, relative, is_default);
            if let Some(p) = install.or(default).filter(|p| p.join("cookies.sqlite").exists()) {
                return Some(p);
            }
        }
        None
    }

    /// Sign in with your everyday Firefox: take its YouTube sign-in, or open
    /// Google's sign-in page in a tab and wait for one. Your browser stays
    /// open.
    pub fn run_everyday(exe: PathBuf, dir: PathBuf, cancel: Arc<AtomicBool>) -> Result<Option<Vec<Cookie>>> {
        let signed_in = |dir: &Path| read_cookies(dir).ok().filter(|c| crate::cookies::signed_in(c));
        if let Some(c) = signed_in(&dir) {
            return Ok(Some(c));
        }
        // Goes to the Firefox that's running, or starts it.
        let mut tab = spawn(Command::new(&exe).args(["--new-tab", SIGN_IN]))?;
        loop {
            let _ = tab.try_wait();
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if signed_in(&dir).is_some() {
                // Let YouTube finish setting its cookies first.
                std::thread::sleep(Duration::from_millis(1500));
                return Ok(signed_in(&dir));
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
    }

    /// Ask Firefox to quit (it saves and closes like File > Quit).
    fn stop(child: &mut Child) {
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = Command::new("taskkill")
                .args(["/T", "/PID", &child.id().to_string()])
                .creation_flags(0x0800_0000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        wait_exit(child, Duration::from_secs(10));
    }

    /// Read the profile's cookies. Firefox may have the database open, so
    /// read a copy (with its write-ahead log, where new cookies land first).
    pub fn read_cookies(dir: &Path) -> Result<Vec<Cookie>> {
        let db = dir.join("cookies.sqlite");
        if !db.exists() {
            return Ok(Vec::new());
        }
        let tmp = crate::auth::data_dir().join("firefox-read");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let result = (|| -> Result<Vec<Cookie>> {
            std::fs::copy(&db, tmp.join("cookies.sqlite"))?;
            let wal = dir.join("cookies.sqlite-wal");
            if wal.exists() {
                std::fs::copy(&wal, tmp.join("cookies.sqlite-wal"))?;
            }
            let conn = rusqlite::Connection::open(tmp.join("cookies.sqlite"))?;
            // Only YouTube / Google cookies, from normal (not container) tabs.
            let mut q = conn.prepare(
                "SELECT host, path, isSecure, isHttpOnly, expiry, name, value FROM moz_cookies \
                 WHERE originAttributes = '' AND (host LIKE '%youtube.com' OR host LIKE '%google.com')",
            )?;
            let rows = q.query_map([], |r| {
                let expiry: i64 = r.get(4)?;
                Ok(Cookie {
                    domain: r.get(0)?,
                    path: r.get(1)?,
                    secure: r.get::<_, i64>(2)? != 0,
                    http_only: r.get::<_, i64>(3)? != 0,
                    // Newer Firefox stores milliseconds.
                    expires: if expiry > 100_000_000_000 { expiry / 1000 } else { expiry },
                    name: r.get(5)?,
                    value: r.get(6)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }
}
