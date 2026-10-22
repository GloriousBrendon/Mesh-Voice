//! Video for Mesh's native helpers: what to share (cameras, screens, windows), capturing it, and
//! H.264 in and out. Windows captures with GDI / Graphics Capture and Media Foundation; Linux with
//! the xdg-desktop-portal ScreenCast picker over PipeWire and V4L2 (see `linux.rs`). Elsewhere
//! listing is empty and capture fails with "not supported on this platform".
//!
//! ```text
//! list_cameras() -> Vec<Camera { id, name }>
//! list_sources() -> Vec<SourceInfo { id, name, kind: Kind::Screen | Kind::Window, thumbnail_jpeg }>
//! Capture::camera(id, Opts { fps, max_width }) -> Result<Capture>   // "" = the first camera
//! Capture::source(id, Opts { fps, max_width }) -> Result<Capture>   // "" = the first screen
//! capture.recv() -> Option<Frame>    // blocks; None once the capture has ended
//! drop(capture)                      // stops it
//! Frame { width, height, rgba, at: Instant }   // even width/height, top-down RGBA
//! frame.jpeg(max_width, quality) -> Result<Vec<u8>>, frame.rotated(quarter_turns) -> Frame
//! H264Encoder::new(fps, bitrate_bps)?.encode(&frame, force_keyframe) -> Result<Vec<u8>>  // Annex B
//! H264Decoder::new()?.decode(annex_b) -> Result<Option<Frame>>, .feed(annex_b) -> Result<()>
//! ```
//!
//! Capture runs on its own thread and keeps at most two frames waiting: a slow consumer gets the
//! newest pictures, not a backlog. The encoder takes any frame size and starts over with a
//! keyframe when it changes (a resized window).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource as _};

#[cfg(windows)]
#[path = "windows.rs"]
mod sys;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod sys;

#[cfg(not(any(windows, target_os = "linux")))]
mod sys {
    use super::*;

    pub fn list_cameras() -> Vec<Camera> {
        Vec::new()
    }
    pub fn list_sources() -> Vec<SourceInfo> {
        Vec::new()
    }
    pub fn open_camera(_: &str, _: Opts) -> Result<Box<dyn Grab>> {
        bail!("camera capture isn't supported on this platform")
    }
    pub fn open_source(_: &str, _: Opts) -> Result<Box<dyn Grab>> {
        bail!("screen capture isn't supported on this platform")
    }
}

/// A camera to capture with [`Capture::camera`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Screen,
    Window,
}

impl Kind {
    /// `"screen"` or `"window"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Screen => "screen",
            Kind::Window => "window",
        }
    }
}

/// A screen or window to share with [`Capture::source`], with a small JPEG of it for a picker.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub thumbnail_jpeg: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    /// Frames a second, at most.
    pub fps: u32,
    /// Wider pictures are scaled down to this (a camera picks its closest mode instead).
    pub max_width: u32,
}

/// One picture: top-down RGBA, `width` and `height` even.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// When it was captured (or decoded).
    pub at: Instant,
}

/// The cameras there are now.
pub fn list_cameras() -> Vec<Camera> {
    sys::list_cameras()
}

/// Screens, then visible top-level windows, each with a thumbnail 240 px wide at most.
pub fn list_sources() -> Vec<SourceInfo> {
    sys::list_sources()
}

/// A running capture; dropping it stops it.
pub struct Capture {
    frames: Receiver<Frame>,
    stop: Arc<AtomicBool>,
}

/// What a capture thread pulls pictures from (built on that thread, so it needn't be `Send`).
pub(crate) trait Grab {
    /// The next picture, waiting for its turn at `Opts::fps`; `None` when there's none yet (asked
    /// again, after a stop check). An error ends capture.
    fn grab(&mut self) -> Result<Option<Frame>>;
}

impl Capture {
    /// Captures a camera by [`Camera::id`]; `""` takes the first one.
    pub fn camera(id: &str, opts: Opts) -> Result<Capture> {
        let id = id.to_owned();
        start(move || sys::open_camera(&id, opts))
    }

    /// Captures a screen or window by [`SourceInfo::id`]; `""` takes the first screen.
    pub fn source(id: &str, opts: Opts) -> Result<Capture> {
        let id = id.to_owned();
        start(move || sys::open_source(&id, opts))
    }

    /// The next frame, waiting for it; `None` once capture has ended (the window closed, the
    /// camera went away).
    pub fn recv(&self) -> Option<Frame> {
        self.frames.recv().ok()
    }

    /// Like [`recv`](Self::recv), giving up after `timeout` (`Ok(None)`); `Err` once it has ended.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Frame>> {
        use std::sync::mpsc::RecvTimeoutError;
        match self.frames.recv_timeout(timeout) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(anyhow!("capture ended")),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn start(open: impl FnOnce() -> Result<Box<dyn Grab>> + Send + 'static) -> Result<Capture> {
    let (tx, frames) = sync_channel(2);
    let (ready_tx, ready) = sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new()
        .name("mesh-media capture".into())
        .spawn(move || {
            let mut source = match open() {
                Ok(source) => {
                    let _ = ready_tx.send(Ok(()));
                    source
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            while !stopped.load(Ordering::Relaxed) {
                match source.grab() {
                    Ok(None) => {}
                    Ok(Some(frame)) => match tx.try_send(frame) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => break,
                    },
                    Err(e) => {
                        log::warn!("capture ended: {e:#}");
                        break;
                    }
                }
            }
        })?;
    ready.recv().map_err(|_| anyhow!("the capture thread died"))??;
    Ok(Capture { frames, stop })
}

/// `w`×`h` scaled to `max_width` at most, both even and at least 2.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn fit(w: u32, h: u32, max_width: u32) -> (u32, u32) {
    let (w, h) = (w.max(2), h.max(2));
    let (w2, h2) = if w > max_width.max(2) { (max_width, (h as u64 * max_width as u64 / w as u64) as u32) } else { (w, h) };
    ((w2 & !1).max(2), (h2 & !1).max(2))
}

/// BGRA rows `pitch` bytes apart, `sw`×`sh`, box-filtered down to `dw`×`dh` RGBA (no larger).
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn shrink(bgra: &[u8], pitch: usize, (sw, sh): (usize, usize), (dw, dh): (usize, usize)) -> Vec<u8> {
    // Each source column and row belongs to one output column and row; sum them, then average.
    let cols: Vec<usize> = (0..sw).map(|x| x * dw / sw).collect();
    let mut widths = vec![0u32; dw];
    cols.iter().for_each(|&c| widths[c] += 1);
    let (mut out, mut sum, mut rows) = (Vec::with_capacity(dw * dh * 4), vec![0u32; dw * 3], 0u32);
    for y in 0..sh {
        let line = &bgra[y * pitch..y * pitch + sw * 4];
        for (px, &c) in line.chunks_exact(4).zip(&cols) {
            let s = &mut sum[c * 3..c * 3 + 3];
            s[0] += px[2] as u32;
            s[1] += px[1] as u32;
            s[2] += px[0] as u32;
        }
        rows += 1;
        if y + 1 == sh || (y + 1) * dh / sh != y * dh / sh {
            for (s, &w) in sum.chunks_exact(3).zip(&widths) {
                let n = (w * rows).max(1);
                out.extend_from_slice(&[(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255]);
            }
            sum.fill(0);
            rows = 0;
        }
    }
    out
}

impl Frame {
    /// The picture as a JPEG, scaled down to `max_width` if wider.
    pub fn jpeg(&self, max_width: u32, quality: u8) -> Result<Vec<u8>> {
        let rgba = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone()).context("bad frame")?;
        let mut img = image::DynamicImage::ImageRgba8(rgba);
        if self.width > max_width {
            img = img.resize(max_width, u32::MAX, image::imageops::FilterType::Triangle);
        }
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&img.to_rgb8())?;
        Ok(out)
    }

    /// Turned clockwise by `quarter_turns` (0–3; others wrap).
    pub fn rotated(&self, quarter_turns: u8) -> Frame {
        use image::imageops::{rotate90, rotate180, rotate270};
        let Some(img) = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone()) else { return self.clone() };
        let img = match quarter_turns & 3 {
            1 => rotate90(&img),
            2 => rotate180(&img),
            3 => rotate270(&img),
            _ => return self.clone(),
        };
        Frame { width: img.width(), height: img.height(), rgba: img.into_raw(), at: self.at }
    }
}

/// H.264 (Constrained Baseline, what OpenH264 makes and WhatsApp/Discord take) from frames.
pub struct H264Encoder {
    encoder: openh264::encoder::Encoder,
    yuv: Option<YUVBuffer>,
}

impl H264Encoder {
    /// An encoder at `fps` and about `bitrate` bits a second, with a keyframe every 2 s.
    pub fn new(fps: u32, bitrate: u32) -> Result<Self> {
        use openh264::encoder::{BitRate, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode};
        let fps = fps.max(1);
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2));
        let encoder = openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config)?;
        Ok(H264Encoder { encoder, yuv: None })
    }

    /// One frame as an Annex-B access unit (SPS and PPS come with each keyframe).
    pub fn encode(&mut self, frame: &Frame, force_keyframe: bool) -> Result<Vec<u8>> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        if w % 2 != 0 || h % 2 != 0 || w == 0 || h == 0 || frame.rgba.len() != w * h * 4 {
            bail!("bad frame: {w}x{h}, {} bytes", frame.rgba.len());
        }
        let rgba = RgbaSliceU8::new(&frame.rgba, (w, h));
        let yuv = match &mut self.yuv {
            Some(yuv) if yuv.dimensions() == (w, h) => {
                yuv.read_rgba8(rgba);
                yuv
            }
            slot => slot.insert(YUVBuffer::from_rgba8_source(rgba)),
        };
        if force_keyframe {
            self.encoder.force_intra_frame();
        }
        Ok(self.encoder.encode(&*yuv)?.to_vec())
    }
}

/// Frames from H.264 (Annex B).
pub struct H264Decoder(openh264::decoder::Decoder);

impl H264Decoder {
    pub fn new() -> Result<Self> {
        Ok(H264Decoder(openh264::decoder::Decoder::new()?))
    }

    /// Decodes an access unit; `None` while the decoder has no picture yet.
    pub fn decode(&mut self, annex_b: &[u8]) -> Result<Option<Frame>> {
        let Some(yuv) = self.0.decode(annex_b)? else { return Ok(None) };
        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Ok(Some(Frame { width: w as u32, height: h as u32, rgba, at: Instant::now() }))
    }

    /// Decodes an access unit without making a picture of it, for frames that won't be shown
    /// (the next ones still need it).
    pub fn feed(&mut self, annex_b: &[u8]) -> Result<()> {
        self.0.decode(annex_b)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, shift: u32) -> Frame {
        let rgba = (0..w * h).flat_map(|i| [((i % w) * 255 / w) as u8, ((i / w + shift) * 255 / h) as u8, 128, 255]).collect();
        Frame { width: w, height: h, rgba, at: Instant::now() }
    }

    #[test]
    fn encodes_and_decodes_back() {
        let mut enc = H264Encoder::new(15, 500_000).unwrap();
        let mut dec = H264Decoder::new().unwrap();
        let first = gradient(320, 240, 0);
        let au = enc.encode(&first, false).unwrap();
        assert_eq!(&au[..4], &[0, 0, 0, 1]);
        let back = dec.decode(&au).unwrap().expect("a picture");
        assert_eq!((back.width, back.height), (320, 240));
        let diff: u64 = first.rgba.iter().zip(&back.rgba).map(|(a, b)| a.abs_diff(*b) as u64).sum();
        assert!(diff / (first.rgba.len() as u64) < 8, "too lossy: {}", diff / first.rgba.len() as u64);
        // A P-frame, then a forced keyframe at a new size (a resized window).
        dec.feed(&enc.encode(&gradient(320, 240, 4), false).unwrap()).unwrap();
        let resized = enc.encode(&gradient(160, 120, 0), true).unwrap();
        let back = dec.decode(&resized).unwrap().expect("a picture");
        assert_eq!((back.width, back.height), (160, 120));
        assert!(enc.encode(&Frame { width: 3, height: 2, rgba: vec![0; 24], at: Instant::now() }, false).is_err());

        let jpeg = back.jpeg(80, 70).unwrap();
        assert_eq!(image::load_from_memory(&jpeg).map(|i| (i.width(), i.height())).unwrap(), (80, 60));
        let turned = back.rotated(1);
        assert_eq!((turned.width, turned.height), (120, 160));
    }

    #[test]
    fn fits_even_sizes() {
        assert_eq!(fit(2560, 1440, 1280), (1280, 720));
        assert_eq!(fit(1001, 333, 1280), (1000, 332));
        assert_eq!(fit(1920, 1081, 1280), (1280, 720));
        assert_eq!(fit(0, 0, 1280), (2, 2));
        // 4x2 BGRA (pitch 20: padded rows) to 2x1: each output pixel averages a 2x2 box.
        let bgra: Vec<u8> = [[10, 20, 30, 0, 30, 40, 50, 0, 1, 1, 1, 0, 3, 3, 3, 0, 9, 9, 9, 9]; 2].concat();
        assert_eq!(shrink(&bgra, 20, (4, 2), (2, 1)), [40, 30, 20, 255, 2, 2, 2, 255]);
        assert_eq!(shrink(&bgra, 20, (4, 2), (4, 2))[..4], [30, 20, 10, 255]);
    }

    #[cfg(windows)]
    #[test]
    fn lists_and_captures_a_screen() {
        let sources = list_sources();
        let screen = sources.iter().find(|s| s.kind == Kind::Screen).expect("a screen");
        assert!(image::load_from_memory(&screen.thumbnail_jpeg).unwrap().width() <= 240);
        let capture = Capture::source(&screen.id, Opts { fps: 10, max_width: 1280 }).unwrap();
        let frame = capture.recv_timeout(Duration::from_secs(5)).unwrap().expect("a frame");
        assert!(frame.width <= 1280 && frame.width % 2 == 0 && frame.height % 2 == 0);
        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);
        assert!(frame.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]), "an all-black screen");
        assert!(Capture::source("window:0", Opts { fps: 10, max_width: 1280 }).is_err());
        if let Some(window) = sources.iter().find(|s| s.kind == Kind::Window) {
            let capture = Capture::source(&window.id, Opts { fps: 10, max_width: 640 }).unwrap();
            let frame = capture.recv_timeout(Duration::from_secs(5)).unwrap().expect("a window frame");
            assert!(frame.width <= 640);
        }
    }

    /// Opens the desktop's share picker (pick something): run by hand with `cargo test -- --ignored`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn captures_a_screen_through_the_portal() {
        assert_eq!(list_sources().len(), 1);
        let capture = Capture::source("", Opts { fps: 15, max_width: 1280 }).unwrap();
        let frames: Vec<Frame> = (0..15).map_while(|_| capture.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        let frame = frames.last().expect("a frame");
        assert!(frame.width <= 1280 && frame.width % 2 == 0 && frame.height % 2 == 0);
        assert!(frame.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]), "an all-black screen");
        let fps = (frames.len() - 1) as f64 / frame.at.duration_since(frames[0].at).as_secs_f64();
        eprintln!("{}x{}, {fps:.1} fps", frame.width, frame.height);
        std::fs::write(std::env::temp_dir().join("mesh-media-screen.jpg"), frame.jpeg(1280, 80).unwrap()).unwrap();
    }

    /// Turns the camera on: run by hand with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn captures_a_camera() {
        let cameras = list_cameras();
        eprintln!("{cameras:?}");
        let capture = Capture::camera("", Opts { fps: 15, max_width: 1280 }).unwrap();
        let frame = capture.recv_timeout(Duration::from_secs(10)).unwrap().expect("a frame");
        eprintln!("{}x{}", frame.width, frame.height);
        std::fs::write(std::env::temp_dir().join("mesh-media-camera.jpg"), frame.jpeg(1280, 80).unwrap()).unwrap();
    }
}
