// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

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
