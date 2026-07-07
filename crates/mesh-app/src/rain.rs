//! An animated "digital rain" background, drawn on an [`iced`] canvas and
//! layered behind the UI. Colors come from the active [`theme::Palette`] so it
//! matches the user's rice; only the motion is fixed.
//!
//! The effect is a pure function of elapsed `time`, so there's no per-frame
//! state to keep — the app just advances `time` on a tick and redraws.

use iced::widget::canvas::{Canvas, Frame, Geometry, Program, Text};
use iced::{Color, Element, Length, Pixels, Point, Rectangle, Renderer, Theme, mouse};

use crate::Message;
use crate::theme;

/// Font-safe glyphs (rendered by the default font — no tofu boxes).
const GLYPHS: &[char] = &[
    '0', '1', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', '<',
    '>', '/', '\\', '=', '+', '*', '#', '$', '%', '@', '?', ':', ';',
];

const COL_WIDTH: f32 = 14.0;
const CELL_HEIGHT: f32 = 16.0;

/// Builds the full-window rain background element for the given palette + time.
pub fn background<'a>(palette: &theme::Palette, time: f32) -> Element<'a, Message> {
    Canvas::new(Rain {
        time,
        head: palette.accent(),
        trail: palette.text_muted(),
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct Rain {
    time: f32,
    head: Color,
    trail: Color,
}

impl Program<Message> for Rain {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        let cols = (bounds.width / COL_WIDTH).ceil() as usize + 1;
        let rows = (bounds.height / CELL_HEIGHT).ceil() as usize + 1;
        let rows_f = rows as f32;

        for col in 0..cols {
            let x = col as f32 * COL_WIDTH;
            // Two staggered streams per column for a denser, livelier field.
            for stream in 0..2u32 {
                let seed = col as u32 * 2 + stream;
                // Per-stream speed (cells/sec) and starting phase, stable over time.
                let speed = 3.0 + (hash(seed) % 90) as f32 / 11.0;
                let trail_len = 7 + (hash(seed ^ 0x5bd1) % 16) as usize;
                let span = rows_f + trail_len as f32;
                let phase = (hash(seed ^ 0x9e37) % 997) as f32 / 997.0 * span;
                let head = (self.time * speed + phase) % span;

                for t in 0..trail_len {
                    let row = head - t as f32;
                    if row < 0.0 || row >= rows_f {
                        continue;
                    }
                    let y = row * CELL_HEIGHT;
                    let ch = glyph(seed, row as u32, self.time);
                    // Head is brightest; the tail fades out.
                    let (base, alpha) = if t == 0 {
                        (self.head, 0.95)
                    } else {
                        (self.trail, (1.0 - t as f32 / trail_len as f32) * 0.6)
                    };
                    frame.fill_text(Text {
                        content: ch.to_string(),
                        position: Point::new(x, y),
                        color: Color { a: alpha, ..base },
                        size: Pixels(CELL_HEIGHT * 0.9),
                        ..Text::default()
                    });
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Cheap deterministic hash (xorshift-ish) for stable per-column randomness.
fn hash(mut x: u32) -> u32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x.wrapping_mul(0x9e3779b1)
}

/// Picks a glyph for a cell; changes slowly over time so streams shimmer.
fn glyph(seed: u32, row: u32, time: f32) -> char {
    let flip = (time * 3.0) as u32;
    let idx = hash(seed.wrapping_mul(31).wrapping_add(row).wrapping_add(flip)) as usize;
    GLYPHS[idx % GLYPHS.len()]
}
