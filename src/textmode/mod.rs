// The engine is public API for the binary: some entry points (rasterise,
// ink_masks) are used by the exporter and tests, not by the live renderer.
#![allow(dead_code)]

//! A text-mode (ANSI / CP437) art engine.
//!
//! The name of the game is fidelity to a machine that no longer exists: an
//! IBM PC in 80x25 text mode, with its 8x16 ROM font, its 16-colour
//! attribute byte, and the iCE convention that spends the blink bit on a
//! background colour. Three things share this core:
//!
//! * [`reader`] decodes `.ANS` / XBin art into a cell grid.
//! * [`writer`] encodes a cell grid back to `.ANS` with SAUCE.
//! * [`reader::from_image`] turns any RGB frame into art, which is how a
//!   render mode and the exporter feed the same machinery.
//!
//! Shared underneath: [`glyph`] holds the real IBM glyph shapes and their
//! Unicode identities, [`palette`] holds the 16 colours, the iCE model and
//! the two-colour cell fit.

pub mod glyph;
pub mod palette;
pub mod reader;
pub mod writer;

pub use glyph::{ch, Font, GLYPH_H, GLYPH_W};
pub use palette::{fit_cell, ink_masks, rasterise, Ice, Rgb, TCell, VGA, XTERM16, XTERM256};
pub use reader::{from_image, read_ansi, to_cells, Art, ReadOpts, Sauce};
pub use writer::{write_ansi, Style, WriteOpts};

/// The standard text-mode screen: 80 columns by 25 rows, the geometry every
/// scrap of classic art was drawn to and every `.ANS` file assumes.
pub const SCREEN_COLS: usize = 80;
pub const SCREEN_ROWS: usize = 25;
