//! `ytui --uninstall`: sign out, then delete ytui's settings, data and cache
//! and the program itself. yt-dlp and ffmpeg stay: other programs may
//! use them.

use anyhow::{bail, Result};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Files a release package puts next to the binary (libmpv-2.dll: older
/// Windows packages).
const PACKAGED: &[&str] = &["libmpv-2.dll", "third-party", "README.md", "LICENSE"];

pub fn run(yes: bool) -> Result<()> {
    let exe = std::env::current_exe().ok();
    // The folder a release package was unpacked into (ytui-<version>-<platform>);
    // only then do the files next to the binary belong to ytui.
    let pkg = exe
        .as_deref()
        .and_then(Path::parent)
        .filter(|d| d.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ytui-")))
        .map(Path::to_path_buf);
    let mut dirs = vec![crate::auth::config_dir(), crate::auth::data_dir(), crate::api::cache_dir()];
    dirs.extend(crate::browser_login::firefox_profiles());
    dirs.dedup();
    dirs.retain(|d| d.exists());
    let packaged: Vec<PathBuf> = pkg.iter().flat_map(|d| PACKAGED.iter().map(move |n| d.join(n))).filter(|p| p.exists()).collect();
    let auth = crate::auth::Auth::load();

    println!("This removes ytui:");
    for p in dirs.iter().chain(&packaged).chain(&exe) {
        println!("  {}", p.display());
    }
    if auth.is_some() {
        println!("and signs you out of Google (revokes ytui's access).");
    }
    println!("yt-dlp and ffmpeg stay installed.");
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("nothing removed; run `ytui --uninstall --yes` to confirm");
        }
        print!("Remove ytui? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Nothing removed.");
            return Ok(());
        }
    }

    if let Some(a) = auth {
        a.sign_out();
        println!("Signed out of Google.");
    }
    let mut failed = 0;
    for p in dirs.iter().chain(&packaged) {
        let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
        if let Err(e) = r {
            eprintln!("could not remove {}: {e}", p.display());
            failed += 1;
        }
    }
    if let Some(exe) = &exe {
        if let Err(e) = remove_self(exe, pkg.as_deref()) {
            eprintln!("could not remove {}: {e}", exe.display());
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} item(s) could not be removed; delete them by hand");
    }
    println!("ytui is uninstalled.");
    Ok(())
}

/// Delete the running binary, then the package folder if that leaves it empty.
#[cfg(unix)]
fn remove_self(exe: &Path, pkg: Option<&Path>) -> std::io::Result<()> {
    std::fs::remove_file(exe)?;
    if let Some(d) = pkg {
        let _ = std::fs::remove_dir(d);
    }
    Ok(())
}

/// Windows can't delete a running .exe, so a detached cmd waits for us to
/// exit and deletes it then (and the package folder, if that leaves it empty).
#[cfg(windows)]
fn remove_self(exe: &Path, pkg: Option<&Path>) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // CREATE_BREAKAWAY_FROM_JOB (our job kills children when we exit, see
    // sys::init) | CREATE_NO_WINDOW.
    const FLAGS: u32 = 0x0100_0000 | 0x0800_0000;
    // Retry for ~20 s: the delete fails until our process has exited.
    let mut script = format!("for /l %i in (1,1,20) do if exist \"{0}\" (ping -n 2 127.0.0.1 >nul & del /f /q \"{0}\" 2>nul)", exe.display());
    if let Some(d) = pkg {
        script = format!("{script} & rmdir \"{}\" 2>nul", d.display());
    }
    std::process::Command::new("cmd")
        .raw_arg(format!("/d /c \"{script}\""))
        .current_dir(std::env::temp_dir())
        .creation_flags(FLAGS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
