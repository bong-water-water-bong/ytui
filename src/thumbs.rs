//! Background image loader (thumbnails, avatars, banners) plus a cache of
//! images already converted to terminal cells for a given size and mode.

use crate::glyphs::{CH, CW};
use crate::render::{Cell, Img, Mode, Renderer};
use image::imageops::FilterType;
use image::RgbImage;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

enum Slot {
    Loading,
    Ready(Arc<RgbImage>),
    Failed,
}

pub struct Thumbs {
    images: Arc<Mutex<HashMap<String, Slot>>>,
    tx: Sender<String>,
    cells: HashMap<(String, u16, u16, Mode), Vec<Cell>>,
}

impl Thumbs {
    /// `wake` is called (from a worker thread) whenever an image arrives.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Thumbs {
        let images: Arc<Mutex<HashMap<String, Slot>>> = Arc::default();
        let (tx, rx) = channel::<String>();
        let rx = Arc::new(Mutex::new(rx));
        let wake = Arc::new(wake);
        for _ in 0..6 {
            let (rx, images, wake) = (rx.clone(), images.clone(), wake.clone());
            std::thread::spawn(move || {
                let agent = crate::api::agent();
                loop {
                    let Ok(url) = rx.lock().unwrap().recv() else { return };
                    let img = agent
                        .get(&url)
                        .call()
                        .ok()
                        .and_then(|r| {
                            let mut buf = Vec::new();
                            std::io::Read::read_to_end(&mut r.into_reader(), &mut buf).ok()?;
                            image::load_from_memory(&buf).ok()
                        })
                        .map(|i| Arc::new(i.to_rgb8()));
                    images.lock().unwrap().insert(url, img.map(Slot::Ready).unwrap_or(Slot::Failed));
                    wake();
                }
            });
        }
        Thumbs { images, tx, cells: HashMap::new() }
    }

    fn image(&self, url: &str) -> Option<Arc<RgbImage>> {
        let mut m = self.images.lock().unwrap();
        match m.get(url) {
            Some(Slot::Ready(i)) => Some(i.clone()),
            Some(_) => None,
            None => {
                if m.len() > 600 {
                    m.retain(|_, s| !matches!(s, Slot::Ready(_)));
                }
                m.insert(url.to_string(), Slot::Loading);
                let _ = self.tx.send(url.to_string());
                None
            }
        }
    }

    /// RGB pixels for `url` cropped to fill exactly pw x ph (None while loading).
    pub fn pixels(&self, url: &str, pw: usize, ph: usize) -> Option<Vec<u8>> {
        let img = self.image(url)?;
        Some(cover(&img, pw as u32, ph as u32).into_raw())
    }

    /// Cells for `url` cropped to fill w x h cells, or None while loading.
    pub fn cells(&mut self, url: &str, w: u16, h: u16, r: &mut Renderer) -> Option<&Vec<Cell>> {
        // Small images read best as mosaics; follow the user's mode only when
        // it is itself a mosaic (e.g. quadrant for fonts without sextants).
        // Half blocks render in every font (sextant glyphs are missing from many).
        let mode = match r.mode {
            Mode::Art | Mode::Ascii | Mode::Classic => Mode::HalfBlock,
            m => m,
        };
        let key = (url.to_string(), w, h, mode);
        if !self.cells.contains_key(&key) {
            let img = self.image(url)?;
            let (pw, ph) = (w as u32 * CW as u32, h as u32 * CH as u32);
            let scaled = cover(&img, pw, ph);
            let mut out = Vec::new();
            let saved = std::mem::replace(&mut r.mode, mode);
            r.render(&Img { w: pw as usize, h: ph as usize, px: scaled.as_raw() }, w as usize, h as usize, &mut out);
            r.mode = saved;
            if self.cells.len() > 500 {
                self.cells.clear();
            }
            self.cells.insert(key.clone(), out);
        }
        self.cells.get(&key)
    }
}

/// Scale-and-crop so the image fills exactly w x h.
fn cover(img: &RgbImage, w: u32, h: u32) -> RgbImage {
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
    let (sw, sh) = (((iw as f64 * scale).round() as u32).max(w), ((ih as f64 * scale).round() as u32).max(h));
    let scaled = image::imageops::resize(img, sw, sh, FilterType::Triangle);
    image::imageops::crop_imm(&scaled, (sw - w) / 2, (sh - h) / 2, w, h).to_image()
}
