//! Playback through libmpv: demuxing, decoding, A/V sync, audio output,
//! seeking and subtitles are mpv's; frames come back through its software
//! render API as RGB and go through ytui's own renderers like any other
//! picture. libmpv is loaded at runtime, so ytui still runs (on the ffmpeg
//! engine) where it isn't installed.

use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

type Handle = *mut c_void;
type RenderCtx = *mut c_void;

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct Event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}

const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const END_EOF: c_int = 0;
const END_ERROR: c_int = 4;
const RP_INVALID: c_int = 0;
const RP_API_TYPE: c_int = 1;
const RP_SW_SIZE: c_int = 17;
const RP_SW_FORMAT: c_int = 18;
const RP_SW_STRIDE: c_int = 19;
const RP_SW_POINTER: c_int = 20;
const UPDATE_FRAME: u64 = 1;

struct Api {
    _lib: Library,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut Event,
    rc_create: unsafe extern "C" fn(*mut RenderCtx, Handle, *mut Param) -> c_int,
    rc_set_update_callback: unsafe extern "C" fn(RenderCtx, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    rc_update: unsafe extern "C" fn(RenderCtx) -> u64,
    rc_render: unsafe extern "C" fn(RenderCtx, *mut Param) -> c_int,
    rc_free: unsafe extern "C" fn(RenderCtx),
}

fn candidates() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("YTUI_LIBMPV") {
        v.push(p.into());
    }
    let names: &[&str] = if cfg!(windows) {
        &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"]
    } else if cfg!(target_os = "macos") {
        &["libmpv.2.dylib", "libmpv.dylib"]
    } else {
        &["libmpv.so.2", "libmpv.so"]
    };
    let mut dirs = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(d);
    }
    if let Some(d) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(std::path::PathBuf::from(d).join("libmpv"));
    }
    for d in &dirs {
        v.extend(names.iter().map(|n| d.join(n)));
    }
    // Bare names: the system's library search path.
    v.extend(names.iter().map(std::path::PathBuf::from));
    v
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let lib = candidates().into_iter().find_map(|p| unsafe { Library::new(&p).ok() })?;
        unsafe {
            macro_rules! sym {
                ($n:literal) => {
                    *lib.get(concat!($n, "\0").as_bytes()).ok()?
                };
            }
            Some(Api {
                create: sym!("mpv_create"),
                initialize: sym!("mpv_initialize"),
                terminate_destroy: sym!("mpv_terminate_destroy"),
                set_option_string: sym!("mpv_set_option_string"),
                set_property_string: sym!("mpv_set_property_string"),
                set_property: sym!("mpv_set_property"),
                get_property: sym!("mpv_get_property"),
                get_property_string: sym!("mpv_get_property_string"),
                command: sym!("mpv_command"),
                error_string: sym!("mpv_error_string"),
                free: sym!("mpv_free"),
                wait_event: sym!("mpv_wait_event"),
                rc_create: sym!("mpv_render_context_create"),
                rc_set_update_callback: sym!("mpv_render_context_set_update_callback"),
                rc_update: sym!("mpv_render_context_update"),
                rc_render: sym!("mpv_render_context_render"),
                rc_free: sym!("mpv_render_context_free"),
                _lib: lib,
            })
        }
    })
    .as_ref()
}

/// Whether libmpv could be loaded.
pub fn available() -> bool {
    api().is_some()
}

fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// What happened since the last poll.
#[derive(Default)]
pub struct Events {
    /// The file is open: tracks can be added.
    pub loaded: bool,
    /// Playback (re)started after loading or a seek: frames are coming.
    pub restarted: bool,
    /// Playback reached the end of the file.
    pub eof: bool,
    /// Playback stopped on an error (message).
    pub error: Option<String>,
}

pub struct Engine {
    api: &'static Api,
    h: Handle,
    rs: Arc<RenderShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Frame number last handed out by `frame`.
    taken: u64,
}

/// State shared with the render thread. Rendering must not happen on the
/// thread that makes blocking client API calls: mpv's core can wait on the
/// renderer (e.g. during video reconfig) while holding what those calls
/// need, and both would wait forever.
struct RenderShared {
    st: Mutex<RenderState>,
    cv: Condvar,
}

#[derive(Default)]
struct RenderState {
    /// mpv asked for a render_context_update.
    wake: bool,
    /// Render now even without a new frame (size change, first frame).
    force: bool,
    quit: bool,
    size: (usize, usize),
    frame: Vec<u8>,
    frame_size: (usize, usize),
    count: u64,
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    let rs = &*(ctx as *const RenderShared);
    rs.st.lock().unwrap().wake = true;
    rs.cv.notify_one();
}

struct SendPtr(RenderCtx);
unsafe impl Send for SendPtr {}

fn render_loop(api: &'static Api, rc: SendPtr, rs: Arc<RenderShared>) {
    let rc = rc.0;
    let mut surf: Vec<u8> = Vec::new();
    let mut rgb: Vec<u8> = Vec::new();
    loop {
        let (force, size) = {
            let mut st = rs.st.lock().unwrap();
            while !st.wake && !st.force && !st.quit {
                // The timeout is only a safety net against a lost wake-up.
                st = rs.cv.wait_timeout(st, std::time::Duration::from_millis(100)).unwrap().0;
                if !st.wake && !st.force && !st.quit {
                    st.wake = true;
                }
            }
            if st.quit {
                break;
            }
            st.wake = false;
            (std::mem::take(&mut st.force), st.size)
        };
        let flags = unsafe { (api.rc_update)(rc) };
        let (w, h) = size;
        if (flags & UPDATE_FRAME == 0 && !force) || w == 0 || h == 0 {
            continue;
        }
        let stride = (w * 4 + 63) & !63;
        // 64-byte aligned surface for mpv's SIMD paths.
        surf.resize(stride * h + 64, 0);
        let off = surf.as_ptr().align_offset(64);
        let mut sz = [w as c_int, h as c_int];
        let fmt = cs("rgb0");
        let mut st = stride;
        let ptr = unsafe { surf.as_mut_ptr().add(off) };
        let mut params = [
            Param { kind: RP_SW_SIZE, data: sz.as_mut_ptr() as *mut c_void },
            Param { kind: RP_SW_FORMAT, data: fmt.as_ptr() as *mut c_void },
            Param { kind: RP_SW_STRIDE, data: &mut st as *mut usize as *mut c_void },
            Param { kind: RP_SW_POINTER, data: ptr as *mut c_void },
            Param { kind: RP_INVALID, data: std::ptr::null_mut() },
        ];
        if unsafe { (api.rc_render)(rc, params.as_mut_ptr()) } < 0 {
            continue;
        }
        rgb.clear();
        rgb.reserve(w * h * 3);
        for row in surf[off..off + stride * h].chunks_exact(stride) {
            for px in row[..w * 4].chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
        }
        let mut st = rs.st.lock().unwrap();
        std::mem::swap(&mut st.frame, &mut rgb);
        st.frame_size = (w, h);
        st.count += 1;
    }
    unsafe { (api.rc_free)(rc) };
}

impl Engine {
    /// `ao`: mpv audio output ("" for mpv's default, "null" for silence).
    /// `wid`: render natively with the GPU into this window instead of
    /// handing frames back.
    pub fn new(ao: &str, wid: Option<i64>) -> Option<Engine> {
        let api = api()?;
        unsafe {
            let h = (api.create)();
            if h.is_null() {
                return None;
            }
            let opt = |k: &str, v: &str| {
                (api.set_option_string)(h, cs(k).as_ptr(), cs(v).as_ptr());
            };
            // mpv must not touch the terminal or take keys: ytui owns both.
            opt("terminal", "no");
            opt("input-default-bindings", "no");
            opt("input-vo-keyboard", "no");
            opt("osc", "no");
            opt("osd-level", "0");
            opt("ytdl", "no");
            match wid {
                Some(w) => {
                    opt("vo", "gpu-next");
                    opt("wid", &w.to_string());
                    opt("hwdec", "auto-safe");
                    opt("input-cursor", "no");
                    opt("input-cursor-passthrough", "yes");
                    opt("cursor-autohide", "no");
                    opt("background-color", "#000000");
                    opt("force-window", "no");
                }
                None => {
                    opt("vo", "libmpv");
                    opt("hwdec", "auto-copy");
                }
            }
            opt("keep-open", "yes");
            opt("idle", "yes");
            opt("volume-max", "150");
            opt("cache", "yes");
            opt("demuxer-max-bytes", "64MiB");
            opt("demuxer-readahead-secs", "20");
            opt("sub-visibility", "no");
            opt("sub-auto", "no");
            opt("audio-display", "no");
            if let Ok(f) = std::env::var("YTUI_MPV_LOG") {
                opt("log-file", &f);
            }
            if !ao.is_empty() {
                opt("ao", ao);
            }
            if (api.initialize)(h) < 0 {
                (api.terminate_destroy)(h);
                return None;
            }
            let rs = Arc::new(RenderShared { st: Mutex::new(RenderState::default()), cv: Condvar::new() });
            if wid.is_some() {
                return Some(Engine { api, h, rs, thread: None, taken: 0 });
            }
            let sw = cs("sw");
            let mut params = [Param { kind: RP_API_TYPE, data: sw.as_ptr() as *mut c_void }, Param { kind: RP_INVALID, data: std::ptr::null_mut() }];
            let mut rc: RenderCtx = std::ptr::null_mut();
            if (api.rc_create)(&mut rc, h, params.as_mut_ptr()) < 0 {
                (api.terminate_destroy)(h);
                return None;
            }
            (api.rc_set_update_callback)(rc, Some(on_update), Arc::as_ptr(&rs) as *mut c_void);
            let (rs2, rcp) = (rs.clone(), SendPtr(rc));
            let thread = std::thread::Builder::new().name("mpv-render".into()).spawn(move || render_loop(api, rcp, rs2)).ok();
            Some(Engine { api, h, rs, thread, taken: 0 })
        }
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cs(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let r = unsafe { (self.api.command)(self.h, ptrs.as_mut_ptr()) };
        if r < 0 {
            Err(unsafe { CStr::from_ptr((self.api.error_string)(r)) }.to_string_lossy().into_owned())
        } else {
            Ok(())
        }
    }

    pub fn set(&self, k: &str, v: &str) {
        unsafe {
            (self.api.set_property_string)(self.h, cs(k).as_ptr(), cs(v).as_ptr());
        }
    }

    pub fn set_flag(&self, k: &str, on: bool) {
        let mut v: c_int = on as c_int;
        unsafe {
            (self.api.set_property)(self.h, cs(k).as_ptr(), FORMAT_FLAG, &mut v as *mut c_int as *mut c_void);
        }
    }

    pub fn set_f64(&self, k: &str, mut v: f64) {
        unsafe {
            (self.api.set_property)(self.h, cs(k).as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void);
        }
    }

    pub fn get_f64(&self, k: &str) -> Option<f64> {
        let mut v = 0f64;
        let r = unsafe { (self.api.get_property)(self.h, cs(k).as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_i64(&self, k: &str) -> Option<i64> {
        let mut v = 0i64;
        let r = unsafe { (self.api.get_property)(self.h, cs(k).as_ptr(), FORMAT_INT64, &mut v as *mut i64 as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_str(&self, k: &str) -> Option<String> {
        unsafe {
            let p = (self.api.get_property_string)(self.h, cs(k).as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.free)(p as *mut c_void);
            Some(s)
        }
    }

    /// Drain mpv's event queue.
    pub fn poll(&self) -> Events {
        let mut ev = Events::default();
        loop {
            let e = unsafe { &*(self.api.wait_event)(self.h, 0.0) };
            if e.event_id != EVENT_NONE && std::env::var_os("YTUI_MPV_DEBUG").is_some() {
                eprintln!("mpv event {}", e.event_id);
            }
            match e.event_id {
                EVENT_NONE => break,
                EVENT_FILE_LOADED => ev.loaded = true,
                EVENT_PLAYBACK_RESTART => ev.restarted = true,
                EVENT_END_FILE if !e.data.is_null() => {
                    let end = unsafe { &*(e.data as *const EndFile) };
                    match end.reason {
                        END_EOF => ev.eof = true,
                        END_ERROR => {
                            let msg = unsafe { CStr::from_ptr((self.api.error_string)(end.error)) }.to_string_lossy().into_owned();
                            ev.error = Some(msg);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        ev
    }

    /// The newest video frame at w x h as packed RGB, swapped into `out`
    /// if there is one we haven't handed out. `force` asks for a render
    /// even without a new frame (e.g. the first one after loading).
    pub fn frame(&mut self, w: usize, h: usize, force: bool, out: &mut Vec<u8>) -> bool {
        let mut st = self.rs.st.lock().unwrap();
        if st.size != (w, h) || force {
            st.size = (w, h);
            st.force = true;
            self.rs.cv.notify_one();
        }
        if st.count != self.taken && st.frame_size == (w, h) {
            self.taken = st.count;
            std::mem::swap(out, &mut st.frame);
            return true;
        }
        false
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // The render context (freed by its thread) must go before the core.
        self.rs.st.lock().unwrap().quit = true;
        self.rs.cv.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe { (self.api.terminate_destroy)(self.h) };
    }
}
