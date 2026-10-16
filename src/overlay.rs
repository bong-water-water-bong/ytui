//! Native video window glued over the terminal (Windows).
//!
//! mpv renders with the GPU into a borderless window that ytui owns. The
//! window is owned by the terminal's top-level window, so it stays above
//! it, minimises with it and never takes focus; mouse input passes
//! through to the terminal. Each UI frame ytui maps the video's cell
//! rectangle to screen pixels and moves the window there.

/// Where the terminal's character grid sits on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Screen position of cell (0, 0)'s top-left corner.
    pub x: i32,
    pub y: i32,
    /// Cell size in physical pixels.
    pub cw: f32,
    pub ch: f32,
}

#[cfg(windows)]
mod imp {
    use super::Grid;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn class_name(h: HWND) -> String {
        let mut b = [0u16; 256];
        let n = unsafe { GetClassNameW(h, b.as_mut_ptr(), b.len() as i32) };
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    /// The terminal's top-level window. Under ConPTY the console window is a
    /// hidden pseudo window owned by the real terminal window.
    pub fn terminal_window() -> Option<HWND> {
        unsafe {
            let c = GetConsoleWindow();
            if c.is_null() {
                return None;
            }
            let owner = GetAncestor(c, GA_ROOTOWNER);
            let h = if !owner.is_null() && owner != c { owner } else { c };
            (IsWindowVisible(h) != 0).then_some(h)
        }
    }

    pub fn client_screen_rect(h: HWND) -> Option<RECT> {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            if GetClientRect(h, &mut r) == 0 {
                return None;
            }
            let mut p = POINT { x: 0, y: 0 };
            ClientToScreen(h, &mut p);
            Some(RECT { left: p.x, top: p.y, right: p.x + r.right, bottom: p.y + r.bottom })
        }
    }

    pub fn dpi(h: HWND) -> u32 {
        unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) }
    }

    unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_ERASEBKGND => {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(h, &mut r);
                FillRect(w as HDC, &r, GetStockObject(BLACK_BRUSH) as HBRUSH);
                1
            }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }

    pub struct Overlay {
        hwnd: AtomicIsize,
        pub owner: HWND,
        shown: Option<(RECT, RECT)>,
    }

    unsafe impl Send for Overlay {}

    impl Overlay {
        /// Create the (hidden) window on its own message-pump thread.
        pub fn new() -> Option<Overlay> {
            let owner = terminal_window()?;
            let (tx, rx) = std::sync::mpsc::channel::<isize>();
            let owner_i = owner as isize;
            std::thread::Builder::new()
                .name("ytui-overlay".into())
                .spawn(move || unsafe {
                    let inst = GetModuleHandleW(std::ptr::null());
                    let cls = wide("ytui-video");
                    let wc = WNDCLASSW {
                        style: CS_HREDRAW | CS_VREDRAW,
                        lpfnWndProc: Some(wndproc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: inst,
                        hIcon: std::ptr::null_mut(),
                        hCursor: std::ptr::null_mut(),
                        hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: cls.as_ptr(),
                    };
                    RegisterClassW(&wc);
                    let title = wide("ytui video");
                    let h = CreateWindowExW(
                        WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                        cls.as_ptr(),
                        title.as_ptr(),
                        WS_POPUP | WS_CLIPCHILDREN,
                        0,
                        0,
                        16,
                        16,
                        owner_i as HWND,
                        std::ptr::null_mut(),
                        inst,
                        std::ptr::null(),
                    );
                    if !h.is_null() {
                        // Layered + fully opaque: lets WS_EX_TRANSPARENT pass clicks through.
                        SetLayeredWindowAttributes(h, 0, 255, LWA_ALPHA);
                    }
                    let _ = tx.send(h as isize);
                    if h.is_null() {
                        return;
                    }
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                })
                .ok()?;
            let h = rx.recv().ok()?;
            (h != 0).then(|| Overlay { hwnd: AtomicIsize::new(h), owner, shown: None })
        }

        /// The window handle, for mpv's `wid`.
        pub fn wid(&self) -> i64 {
            self.hwnd.load(Ordering::Relaxed) as i64
        }

        /// Show the window over screen rectangle `full`, clipped to `vis`
        /// (the part on screen), or hide it.
        pub fn place(&mut self, r: Option<(RECT, RECT)>) {
            let h = self.hwnd.load(Ordering::Relaxed) as HWND;
            let eq = |a: &RECT, b: &RECT| a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom;
            let same = match (&self.shown, &r) {
                (Some(a), Some(b)) => eq(&a.0, &b.0) && eq(&a.1, &b.1),
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            unsafe {
                match r {
                    Some((r, v)) => {
                        let clip = !eq(&r, &v);
                        let rgn = if clip { CreateRectRgn(v.left - r.left, v.top - r.top, v.right - r.left, v.bottom - r.top) } else { std::ptr::null_mut() };
                        SetWindowRgn(h, rgn, 1);
                        SetWindowPos(
                            h,
                            std::ptr::null_mut(),
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
                        );
                    }
                    None => {
                        ShowWindow(h, SW_HIDE);
                    }
                }
            }
            self.shown = r;
        }

        /// Screen rectangle of a cell box, given the grid geometry.
        pub fn cells_to_screen(g: Grid, x: u16, y: u16, w: u16, h: u16) -> RECT {
            let l = g.x + (x as f32 * g.cw).round() as i32;
            let t = g.y + (y as f32 * g.ch).round() as i32;
            RECT { left: l, top: t, right: g.x + ((x + w) as f32 * g.cw).round() as i32, bottom: g.y + ((y + h) as f32 * g.ch).round() as i32 }
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            let h = self.hwnd.load(Ordering::Relaxed) as HWND;
            unsafe {
                PostMessageW(h, WM_CLOSE, 0, 0);
            }
        }
    }
}

#[cfg(windows)]
pub use imp::*;

/// Diagnostic: what the terminal window looks like from here.
pub fn probe(out: &str) -> anyhow::Result<()> {
    use std::fmt::Write as _;
    let mut s = String::new();
    let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
    let _ = writeln!(s, "grid {cols}x{rows}");
    let caps = crate::gfx::detect();
    let _ = writeln!(s, "caps {caps:?}");
    let reply = crate::sys::query_tty(b"\x1b[14t\x1b[16t\x1b[18t\x1b[c", |b| b.windows(3).any(|w| w == b"\x1b[?") && b.ends_with(b"c"), 800);
    let _ = writeln!(s, "xtwinops {:?}", String::from_utf8_lossy(&reply));
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let c = GetConsoleWindow();
        let _ = writeln!(s, "console hwnd {:?} class {:?}", c, class_name(c));
        let o = GetAncestor(c, GA_ROOTOWNER);
        let _ = writeln!(s, "root owner {:?} class {:?}", o, class_name(o));
        if let Some(t) = terminal_window() {
            let _ = writeln!(s, "terminal {:?} class {:?} dpi {} client {:?}", t, class_name(t), dpi(t), client_screen_rect(t).map(|r| (r.left, r.top, r.right, r.bottom)));
            let mut wr: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            GetWindowRect(t, &mut wr);
            let _ = writeln!(s, "window rect {:?}", (wr.left, wr.top, wr.right, wr.bottom));
            let _ = writeln!(s, "uia grid {:?}", uia_grid(t, rows));
        }
    }
    std::fs::write(out, s)?;
    Ok(())
}

/// Exact grid geometry from Windows Terminal's UI Automation text pattern:
/// the bounding boxes of the first visible line and of its first character.
#[cfg(windows)]
pub fn uia_grid(term: windows_sys::Win32::Foundation::HWND, rows: u16) -> Option<Grid> {
    use windows::core::Interface;
    use windows::Win32::Foundation::HWND as WHWND;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Accessibility::*;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let root = uia.ElementFromHandle(WHWND(term as _)).ok()?;
        let cond = uia.CreatePropertyCondition(UIA_ClassNamePropertyId, &windows::core::VARIANT::from("TermControl")).ok()?;
        // Only the showing tab's controls are in the tree; with split panes
        // prefer the focused one. Panes of one tab share the cell size.
        let all = root.FindAll(TreeScope_Descendants, &cond).ok()?;
        let mut el = None;
        for i in 0..all.Length().ok()? {
            let e = all.GetElement(i).ok()?;
            if e.CurrentIsOffscreen().is_ok_and(|o| o.as_bool()) {
                continue;
            }
            let focused = e.CurrentHasKeyboardFocus().is_ok_and(|f| f.as_bool());
            if el.is_none() || focused {
                el = Some(e);
            }
            if focused {
                break;
            }
        }
        let el = el?;
        let tp: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let vis = tp.GetVisibleRanges().ok()?;
        let line = vis.GetElement(0).ok()?;
        let rects = |r: &IUIAutomationTextRange| -> Option<Vec<f64>> {
            let sa = r.GetBoundingRectangles().ok()?;
            let n = (*sa).rgsabound[0].cElements as usize;
            let data = std::slice::from_raw_parts((*sa).pvData as *const f64, n).to_vec();
            let _ = windows::Win32::System::Ole::SafeArrayDestroy(sa);
            Some(data)
        };
        let first = line.Clone().ok()?;
        first.ExpandToEnclosingUnit(TextUnit_Line).ok()?;
        let lr = rects(&first)?;
        let ch_r = line.Clone().ok()?;
        ch_r.ExpandToEnclosingUnit(TextUnit_Character).ok()?;
        let cr = rects(&ch_r)?;
        let _ = rows;
        let _ = el.cast::<IUIAutomationElement>();
        if lr.len() < 4 || cr.len() < 4 || cr[2] <= 0.0 || lr[3] <= 0.0 {
            return None;
        }
        Some(Grid { x: lr[0].round() as i32, y: lr[1].round() as i32, cw: cr[2] as f32, ch: lr[3] as f32 })
    }
}

/// Whether the terminal window is the foreground window.
#[cfg(windows)]
pub fn is_foreground(term: windows_sys::Win32::Foundation::HWND) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == term }
}

/// The terminal grid's screen geometry, or None when the terminal window
/// is minimised or the grid can't be found (e.g. another tab is active).
#[cfg(windows)]
pub fn current_grid(term: windows_sys::Win32::Foundation::HWND) -> Option<Grid> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    unsafe {
        if IsIconic(term) != 0 || IsWindowVisible(term) == 0 {
            return None;
        }
    }
    let (_, rows) = crossterm::terminal::size().ok()?;
    uia_grid(term, rows)
}
