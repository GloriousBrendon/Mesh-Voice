//! Animated backgrounds ("digital rain", a drifting mesh, stars), drawn on an
//! [`iced`] canvas and layered behind the UI. Colors come from the active
//! [`theme::Palette`] so they match the user's rice; only the motion is fixed.
//!
//! Each is a pure function of elapsed `time`, so there's no per-frame state to
//! keep — the app just advances `time` on a tick and redraws.

use iced::widget::canvas::{Canvas, Frame, Geometry, Path, Program, Stroke, Text};
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

/// The full-window animated background for the given palette + time.
pub fn background<'a>(palette: &theme::Palette, time: f32, animation: theme::Animation) -> Element<'a, Message> {
    let (accent, muted) = (palette.accent(), palette.text_muted());
    let canvas: Element<'a, Message> = match animation {
        theme::Animation::Rain => Canvas::new(Rain { time, head: accent, trail: muted }).width(Length::Fill).height(Length::Fill).into(),
        theme::Animation::Mesh => Canvas::new(Mesh { time, node: accent, link: muted }).width(Length::Fill).height(Length::Fill).into(),
        theme::Animation::Stars => Canvas::new(Stars { time, bright: accent, dim: muted }).width(Length::Fill).height(Length::Fill).into(),
    };
    canvas
}

/// Distance at which mesh nodes stop being linked.
const LINK: f32 = 150.0;

/// Nodes drifting across the window (wrapping at the edges), linked by lines
/// that fade with distance: the app's name, drawn.
struct Mesh {
    time: f32,
    node: Color,
    link: Color,
}

impl Program<Message> for Mesh {
    type State = ();

    fn draw(&self, _state: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));
        // About one node per 160x160 px, capped: links are checked pairwise.
        let count = ((w * h) / 26_000.0).clamp(12.0, 110.0) as u32;
        let nodes: Vec<Point> = (0..count)
            .map(|i| {
                let unit = |salt: u32| (hash(i.wrapping_mul(7919) ^ salt) % 10_000) as f32 / 10_000.0;
                // 4–16 px a second, in any direction.
                let (speed, angle) = (4.0 + unit(0x51) * 12.0, unit(0xa3) * std::f32::consts::TAU);
                let x = (unit(0x17) * w + angle.cos() * speed * self.time).rem_euclid(w);
                let y = (unit(0x2b) * h + angle.sin() * speed * self.time).rem_euclid(h);
                Point::new(x, y)
            })
            .collect();
        for (i, a) in nodes.iter().enumerate() {
            for b in &nodes[i + 1..] {
                let d = a.distance(*b);
                if d < LINK {
                    let fade = 1.0 - d / LINK;
                    frame.stroke(&Path::line(*a, *b), Stroke::default().with_width(1.0).with_color(Color { a: fade * 0.5, ..self.link }));
                }
            }
        }
        for (i, p) in nodes.iter().enumerate() {
            // A slow pulse, out of step from node to node.
            let pulse = 0.55 + 0.35 * (self.time * 1.3 + i as f32).sin();
            frame.fill(&Path::circle(*p, 2.2), Color { a: pulse, ..self.node });
        }
        vec![frame.into_geometry()]
    }
}

/// Three layers of stars drifting left at different speeds, twinkling.
struct Stars {
    time: f32,
    bright: Color,
    dim: Color,
}

impl Program<Message> for Stars {
    type State = ();

    fn draw(&self, _state: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));
        let per_layer = ((w * h) / 9_000.0).clamp(30.0, 400.0) as u32;
        for (layer, (speed, size)) in [(3.0, 0.8), (7.0, 1.2), (14.0, 1.8)].into_iter().enumerate() {
            for i in 0..per_layer {
                let seed = (layer as u32) << 20 | i;
                let unit = |salt: u32| (hash(seed.wrapping_mul(2654435761) ^ salt) % 10_000) as f32 / 10_000.0;
                let x = (unit(0x11) * w - speed * self.time).rem_euclid(w);
                let y = unit(0x22) * h;
                let twinkle = 0.45 + 0.4 * (self.time * (0.6 + unit(0x33) * 2.0) + unit(0x44) * 6.3).sin();
                // One star in twelve in the accent color.
                let color = if hash(seed ^ 0x55) % 12 == 0 { self.bright } else { self.dim };
                frame.fill(&Path::circle(Point::new(x, y), size), Color { a: twinkle * (0.5 + layer as f32 * 0.2), ..color });
            }
        }
        vec![frame.into_geometry()]
    }
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
