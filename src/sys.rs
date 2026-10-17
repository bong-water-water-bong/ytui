//! The few things that differ between Unix and Windows: tying child
//! processes to our lifetime, freezing ffmpeg for pause, private files,
//! opening a browser, and reading terminal replies during capability
//! detection.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// The user's home directory ($HOME, or %USERPROFILE% on Windows).
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Whether `cmd` is an executable on PATH.
pub fn on_path(cmd: &str) -> bool {
    let exe = if cfg!(windows) { format!("{cmd}.exe") } else { cmd.to_string() };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&exe).is_file()))
}

/// Write a file only the user can read (mode 0600 on Unix; on Windows the
/// profile directory's ACL already restricts it), replacing it atomically.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Open a URL in the desktop's browser, if there is a desktop.
pub fn open_url(url: &str) {
    use std::process::Stdio;
    #[cfg(unix)]
    {
        if std::env::var("SSH_CONNECTION").is_err() && std::env::var("DISPLAY").is_ok() {
            let _ = Command::new("xdg-open").arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_BREAKAWAY_FROM_JOB: the browser must outlive us (see init).
        const BREAKAWAY: u32 = 0x0100_0000;
        if std::env::var("SSH_CONNECTION").is_err() {
            let _ = Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", url])
                .creation_flags(BREAKAWAY)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

/// Process-wide setup. On Windows, put ourselves in a job object that kills
/// everything in it when the last handle closes (i.e. when we exit, however
/// we exit), the equivalent of PR_SET_PDEATHSIG for every child.
pub fn init() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::JobObjects::*;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok != 0 {
            AssignProcessToJobObject(job, GetCurrentProcess());
        }
        // The handle is deliberately leaked: closing it is what kills the job.
    }
}

/// Have the child die if we die (terminal closed, SIGKILL), so ffmpeg /
/// audio players never outlive us. On Windows the job object from `init`
/// already covers every child.
pub fn die_with_parent(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                #[cfg(target_os = "linux")]
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    let _ = cmd;
}

#[cfg(windows)]
#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(h: windows_sys::Win32::Foundation::HANDLE) -> i32;
    fn NtResumeProcess(h: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

/// Freeze or thaw a child process in place (SIGSTOP / SIGCONT).
pub fn suspend(child: &Child, paused: bool) {
    #[cfg(unix)]
    unsafe {
        let sig = if paused { libc::SIGSTOP } else { libc::SIGCONT };
        libc::kill(child.id() as libc::pid_t, sig);
    }
    #[cfg(windows)]
    unsafe {
        use std::os::windows::io::AsRawHandle;
        let h = child.as_raw_handle() as _;
        if paused {
            NtSuspendProcess(h);
        } else {
            NtResumeProcess(h);
        }
    }
}

/// Whether a process with this id is still running.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Cell size in pixels from the kernel, when the terminal reports it.
pub fn cell_px() -> Option<(u16, u16)> {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd;
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(std::io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
            return Some((ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row));
        }
    }
    None
}

/// Write `q` to the terminal and collect its replies until `done` says they
/// are complete or `ms` elapses. Reads raw bytes, bypassing crossterm's
/// event parser.
pub fn query_tty(q: &[u8], done: impl Fn(&[u8]) -> bool, ms: u64) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut got = Vec::new();
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if crossterm::terminal::enable_raw_mode().is_err() {
            return got;
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(q);
        let _ = out.flush();
        let fd = std::io::stdin().as_raw_fd();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now()).as_millis() as i32;
            if left <= 0 {
                break;
            }
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pfd, 1, left) } <= 0 {
                break;
            }
            let mut b = [0u8; 1024];
            let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, b.len()) };
            if n <= 0 {
                break;
            }
            got.extend_from_slice(&b[..n as usize]);
            if done(&got) {
                break;
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
    }
    #[cfg(windows)]
    unsafe {
        // Replies arrive as key events once VT input is on; collect their
        // characters.
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Console::*;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        let hin: HANDLE = GetStdHandle(STD_INPUT_HANDLE);
        let hout: HANDLE = GetStdHandle(STD_OUTPUT_HANDLE);
        let (mut im, mut om) = (0u32, 0u32);
        if GetConsoleMode(hin, &mut im) == 0 || GetConsoleMode(hout, &mut om) == 0 {
            return got;
        }
        SetConsoleMode(hin, ENABLE_VIRTUAL_TERMINAL_INPUT);
        SetConsoleMode(hout, om | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        FlushConsoleInputBuffer(hin);
        let mut out = std::io::stdout();
        let _ = out.write_all(q);
        let _ = out.flush();
        let mut units: Vec<u16> = Vec::new();
        let mut recs: [INPUT_RECORD; 128] = std::mem::zeroed();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now()).as_millis() as u32;
            if left == 0 || WaitForSingleObject(hin, left) != WAIT_OBJECT_0 {
                break;
            }
            let mut n = 0u32;
            if ReadConsoleInputW(hin, recs.as_mut_ptr(), recs.len() as u32, &mut n) == 0 {
                break;
            }
            for r in &recs[..n as usize] {
                if r.EventType as u32 == KEY_EVENT {
                    let k = r.Event.KeyEvent;
                    let c = k.uChar.UnicodeChar;
                    if k.bKeyDown != 0 && c != 0 {
                        units.push(c);
                    }
                }
            }
            got = String::from_utf16_lossy(&units).into_bytes();
            if done(&got) {
                break;
            }
        }
        SetConsoleMode(hin, im);
        SetConsoleMode(hout, om);
    }
    got
}

/// Console output mode bits and crossterm's ANSI verdict, for diagnostics.
pub fn console_state() -> String {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::*;
        let (mut o, mut i) = (0u32, 0u32);
        GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut o);
        GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut i);
        return format!("out {o:#x} in {i:#x} ansi {}", crossterm::ansi_support::supports_ansi());
    }
    #[allow(unreachable_code)]
    String::new()
}
