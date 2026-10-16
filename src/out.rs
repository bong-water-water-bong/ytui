//! Terminal output on its own thread. A frame (text diff + pixel graphics)
//! is assembled in memory and handed to a writer thread, so the next frame
//! decodes, renders and encodes while the terminal is still swallowing the
//! last one. At most one frame is in flight: handing over a frame waits
//! for the writer to be free, which keeps latency to one frame and lets a
//! slow terminal pace the loop instead of piling up output.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
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

pub struct Out {
    buf: Buf,
    tx: SyncSender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
}

impl Out {
    pub fn new() -> (Out, Buf) {
        let buf = Buf::default();
        let (tx, rx) = sync_channel::<Vec<u8>>(0);
        let pending = Arc::new(AtomicUsize::new(0));
        let p = pending.clone();
        std::thread::Builder::new()
            .name("ytui-out".into())
            .spawn(move || {
                let mut o = std::io::stdout();
                for v in rx {
                    let _ = o.write_all(&v);
                    let _ = o.flush();
                    p.fetch_sub(1, Ordering::Release);
                }
            })
            .expect("output thread");
        (Out { buf: buf.clone(), tx, pending }, buf)
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
