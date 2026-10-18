// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

//! Terminal output on its own thread. A frame (text diff + pixel graphics)
//! is assembled in memory and handed to a writer thread, so the next frame
//! decodes, renders and encodes while the terminal is still swallowing the
//! last one. At most one frame is in flight: handing over a frame waits
//! for the writer to be free, which keeps latency to one frame and lets a
//! slow terminal pace the loop instead of piling up output.
//!
//! A write returning doesn't mean the terminal has caught up: VTE and
//! others read the pty eagerly into their own queue, so frames can pile up
//! inside the terminal and the picture lags further and further. After each
//! big frame the writer asks for the cursor position, which the terminal
//! can only answer once it has processed everything before it; until the
//! answer comes the writer stays busy, so the app skips frames instead of
//! queueing them. Bytes over that round trip is the terminal's real rate,
//! which the video renderer sizes each frame to fit.

use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};

/// In-memory sink for ratatui's backend; `Out::push` sends what it holds.
#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Share of the terminal's measured rate a video frame may use; the rest
/// is headroom for the UI around it and for jitter.
pub const BUDGET_SHARE: f64 = 0.7;

/// Rate assumed before anything is measured: about half of what VTE
/// (GNOME Terminal, Ptyxis) parses.
const START_RATE: f64 = 8e6;

/// Frames smaller than this go out without a round trip.
const MEASURE_MIN: usize = 32 * 1024;

pub struct Out {
    buf: Buf,
    tx: SyncSender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
    /// Measured terminal throughput in bytes per second (f64 bits).
    rate: Arc<AtomicU64>,
}

impl Out {
    pub fn new() -> (Out, Buf) {
        let buf = Buf::default();
        let (tx, rx) = sync_channel::<Vec<u8>>(0);
        let pending = Arc::new(AtomicUsize::new(0));
        let p = pending.clone();
        let rate = Arc::new(AtomicU64::new(START_RATE.to_bits()));
        let r = rate.clone();
        std::thread::Builder::new()
            .name("ytui-out".into())
            .spawn(move || {
                let mut o = std::io::stdout();
                // Windows' console answers the position without the terminal.
                let mut probe = cfg!(unix) && std::env::var_os("YTUI_NO_PROBE").is_none();
                for v in rx {
                    let t = std::time::Instant::now();
                    let _ = o.write_all(&v);
                    let _ = o.flush();
                    if v.len() >= MEASURE_MIN && probe {
                        probe = crossterm::cursor::position().is_ok();
                    }
                    let secs = t.elapsed().as_secs_f64();
                    if v.len() >= MEASURE_MIN && secs > 0.0 {
                        let old = f64::from_bits(r.load(Ordering::Relaxed));
                        let now = (v.len() as f64 / secs).clamp(1e6, 200e6);
                        r.store((old * 0.8 + now * 0.2).to_bits(), Ordering::Relaxed);
                    }
                    p.fetch_sub(1, Ordering::Release);
                }
            })
            .expect("output thread");
        (Out { buf: buf.clone(), tx, pending, rate }, buf)
    }

    /// Send everything buffered so far, plus `extra`, as one write. Waits
    /// only until the writer has taken the previous frame.
    pub fn push(&self, extra: &[u8]) {
        let mut v = std::mem::take(&mut *self.buf.0.lock().unwrap());
        v.extend_from_slice(extra);
        if v.is_empty() {
            return;
        }
        self.pending.fetch_add(1, Ordering::Acquire);
        if self.tx.send(v).is_err() {
            self.pending.fetch_sub(1, Ordering::Release);
        }
    }

    /// The terminal's measured throughput in bytes per second.
    pub fn rate(&self) -> f64 {
        f64::from_bits(self.rate.load(Ordering::Relaxed))
    }

    /// The writer is done with everything sent.
    pub fn busy(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }

    /// Wait until everything sent has reached the terminal.
    pub fn drain(&self) {
        while self.busy() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
