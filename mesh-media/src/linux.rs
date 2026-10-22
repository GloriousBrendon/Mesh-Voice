//! Linux capture. Screens and windows go through the desktop's own picker (xdg-desktop-portal
//! ScreenCast, frames over PipeWire): Wayland lets no app grab the screen without it, so there's
//! one source to list and the portal asks which screen or window. Cameras are V4L2 devices.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use pipewire as pw;
use pw::spa;
use v4l::io::traits::CaptureStream as _;
use v4l::video::Capture as _;

use crate::{Camera, Frame, Grab, Kind, Opts, SourceInfo, fit, shrink};

/// The one source listed: picking it opens the portal's picker.
pub fn list_sources() -> Vec<SourceInfo> {
    vec![SourceInfo { id: "portal".into(), name: "Choose a screen or window…".into(), kind: Kind::Screen, thumbnail_jpeg: Vec::new() }]
}

/// What the PipeWire callbacks hand to `grab`.
#[derive(Default)]
struct Shared {
    format: spa::param::video::VideoInfoRaw,
    newest: Option<Frame>,
    taken: Option<Instant>,
    ended: Option<String>,
}

struct Screen {
    // Field order is drop order: the stream goes before its core and loop.
    _listener: pw::stream::StreamListener<Rc<RefCell<Shared>>>,
    stream: pw::stream::StreamRc,
    _core: pw::core::CoreRc,
    _context: pw::context::ContextRc,
    mainloop: pw::main_loop::MainLoopRc,
    shared: Rc<RefCell<Shared>>,
    session: Session<Screencast>,
    gap: Duration,
    next: Instant,
    last: Option<Frame>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
        // Ends the share (and the desktop's "sharing" indicator) now, not when the helper exits.
        let _ = async_io::block_on(self.session.close());
    }
}

/// Any `id` (there's only the portal entry) asks the portal; blocks until the user picks.
pub fn open_source(_id: &str, opts: Opts) -> Result<Box<dyn Grab>> {
    let (session, node, fd) = async_io::block_on(async {
        let portal = Screencast::new().await.context("no screen-sharing portal (xdg-desktop-portal)")?;
        let session = portal.create_session(Default::default()).await.context("portal: create session")?;
        let cursor = match portal.available_cursor_modes().await {
            Ok(modes) if modes.contains(CursorMode::Embedded) => CursorMode::Embedded,
            _ => CursorMode::Hidden,
        };
        let sources = SelectSourcesOptions::default()
            .set_cursor_mode(cursor)
            .set_sources(SourceType::Monitor | SourceType::Window)
            .set_multiple(false)
            .set_persist_mode(PersistMode::DoNot);
        portal.select_sources(&session, sources).await.context("portal: select sources")?;
        let started = portal.start(&session, None, Default::default()).await.context("portal: start")?.response().context("sharing was cancelled")?;
        let node = started.streams().first().context("nothing was picked")?.pipe_wire_node_id();
        let fd = portal.open_pipe_wire_remote(&session, Default::default()).await.context("portal: PipeWire remote")?;
        anyhow::Ok((session, node, fd))
    })?;

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "mesh-media",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;
    let gap = Duration::from_secs(1) / opts.fps.max(1);
    let shared = Rc::new(RefCell::new(Shared::default()));
    let listener = stream
        .add_local_listener_with_user_data(shared.clone())
        .state_changed(|_, shared, _, state| match state {
            pw::stream::StreamState::Error(e) => shared.borrow_mut().ended = Some(e),
            pw::stream::StreamState::Unconnected => shared.borrow_mut().ended = Some("the share ended".into()),
            _ => {}
        })
        .param_changed(|_, shared, id, param| {
            let Some(param) = param else { return };
            if id == spa::param::ParamType::Format.as_raw() {
                let _ = shared.borrow_mut().format.parse(param);
            }
        })
        .process(move |stream, shared| {
            // Dequeued buffers go back to PipeWire when dropped.
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let mut shared = shared.borrow_mut();
            // The screen sends at its refresh rate (165 Hz…): only copy the pictures we'll use.
            if shared.taken.is_some_and(|t| t.elapsed() < gap * 3 / 4) {
                return;
            }
            let (format, size) = (shared.format.format(), shared.format.size());
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let (offset, len, stride) = (data.chunk().offset() as usize, data.chunk().size() as usize, data.chunk().stride());
            let (w, h) = (size.width as usize, size.height as usize);
            let pitch = if stride > 0 { stride as usize } else { w * 4 };
            let Some(bytes) = data.data().and_then(|d| d.get(offset..offset + len)) else { return };
            if w == 0 || h == 0 || bytes.len() < pitch * (h - 1) + w * 4 {
                return;
            }
            use spa::param::video::VideoFormat as F;
            let bgr = match format {
                F::BGRx | F::BGRA => true,
                F::RGBx | F::RGBA => false,
                _ => return,
            };
            let (dw, dh) = fit(w as u32, h as u32, opts.max_width);
            let mut rgba = shrink(bytes, pitch, (w, h), (dw as usize, dh as usize));
            if !bgr {
                rgba.chunks_exact_mut(4).for_each(|px| px.swap(0, 2));
            }
            shared.newest = Some(Frame { width: dw, height: dh, rgba, at: Instant::now() });
            shared.taken = Some(Instant::now());
        })
        .register()?;

    let format = video_format_pod(opts.fps)?;
    let mut params = [spa::pod::Pod::from_bytes(&format).context("bad format pod")?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    Ok(Box::new(Screen { _listener: listener, stream, _core: core, _context: context, mainloop, shared, session, gap, next: Instant::now(), last: None }))
}

/// Raw video in 32-bit RGB orders (memory buffers, not DMA-BUF), any size, about `fps`.
fn video_format_pod(fps: u32) -> Result<Vec<u8>> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::utils::{Fraction, Rectangle};
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps.max(1), denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    let (cursor, _) = spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(object))
        .map_err(|e| anyhow!("format pod: {e:?}"))?;
    Ok(cursor.into_inner())
}

impl Grab for Screen {
    fn grab(&mut self) -> Result<Option<Frame>> {
        // Run PipeWire until the next picture is due, then hand over the newest one; an
        // unchanged screen sends nothing, so the last picture repeats (as on Windows).
        let lp = self.mainloop.loop_();
        loop {
            let now = Instant::now();
            if now >= self.next {
                break;
            }
            lp.iterate(pw::loop_::Timeout::Finite(self.next - now));
        }
        while lp.iterate(pw::loop_::Timeout::None) > 0 {}
        self.next = self.next.max(Instant::now()) + self.gap;
        let mut shared = self.shared.borrow_mut();
        if let Some(why) = shared.ended.take() {
            bail!("{why}");
        }
        if let Some(frame) = shared.newest.take() {
            self.last = Some(frame);
        }
        Ok(self.last.as_ref().map(|f| Frame { at: Instant::now(), ..f.clone() }))
    }
}

/// Video capture devices, `/dev/video0` first (a webcam's metadata node isn't one).
pub fn list_cameras() -> Vec<Camera> {
    let mut found: Vec<(u32, Camera)> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let n: u32 = name.strip_prefix("video")?.parse().ok()?;
            let caps = v4l::Device::new(n as usize).ok()?.query_caps().ok()?;
            caps.capabilities.contains(v4l::capability::Flags::VIDEO_CAPTURE).then(|| (n, Camera { id: format!("/dev/{name}"), name: caps.card }))
        })
        .collect();
    found.sort_by_key(|(n, _)| *n);
    found.into_iter().map(|(_, c)| c).collect()
}

const MJPG: &[u8; 4] = b"MJPG";
const YUYV: &[u8; 4] = b"YUYV";

struct Cam {
    stream: v4l::prelude::MmapStream<'static>,
    mjpeg: bool,
    w: u32,
    h: u32,
    gap: Duration,
    last: Option<Instant>,
}

pub fn open_camera(id: &str, opts: Opts) -> Result<Box<dyn Grab>> {
    let path = match id {
        "" => list_cameras().into_iter().next().context("no camera found")?.id,
        id => id.to_owned(),
    };
    let dev = v4l::Device::with_path(&path).with_context(|| format!("couldn't open {path} (in use, or no permission?)"))?;
    // MJPEG first: it's how webcams send their larger sizes at full speed.
    let formats: Vec<_> = dev.enum_formats()?.into_iter().map(|f| f.fourcc).collect();
    let fourcc = [MJPG, YUYV]
        .into_iter()
        .map(v4l::FourCC::new)
        .find(|f| formats.contains(f))
        .context("the camera sends neither MJPEG nor YUYV")?;
    let (w, h) = best_size(&dev.enum_framesizes(fourcc).unwrap_or_default(), opts.max_width).unwrap_or((640, 480));
    let set = dev.set_format(&v4l::Format::new(w, h, fourcc))?;
    if set.fourcc != fourcc {
        bail!("the camera wouldn't switch to {fourcc}");
    }
    let _ = dev.set_params(&v4l::video::capture::Parameters::with_fps(opts.fps.max(1)));
    let stream = v4l::prelude::MmapStream::with_buffers(&dev, v4l::buffer::Type::VideoCapture, 4)?;
    let gap = Duration::from_secs(1) / opts.fps.max(1);
    Ok(Box::new(Cam { stream, mjpeg: fourcc == v4l::FourCC::new(MJPG), w: set.width, h: set.height, gap, last: None }))
}

/// The widest size that fits `max_width`, else the narrowest there is.
fn best_size(sizes: &[v4l::framesize::FrameSize], max_width: u32) -> Option<(u32, u32)> {
    use v4l::framesize::FrameSizeEnum;
    let all: Vec<(u32, u32)> = sizes
        .iter()
        .flat_map(|s| match &s.size {
            FrameSizeEnum::Discrete(d) => vec![(d.width, d.height)],
            // Any size in a range: its ends are enough to choose from.
            FrameSizeEnum::Stepwise(r) => vec![(r.min_width, r.min_height), (r.max_width, r.max_height)],
        })
        .collect();
    all.iter().filter(|(w, _)| *w <= max_width).max().or_else(|| all.iter().min()).copied()
}

impl Grab for Cam {
    fn grab(&mut self) -> Result<Option<Frame>> {
        loop {
            let (buf, meta) = self.stream.next().context("the camera stopped")?;
            // A 30 fps camera at 15: take every other frame, not every third.
            if self.last.is_some_and(|t| t.elapsed() < self.gap * 3 / 4) {
                continue;
            }
            self.last = Some(Instant::now());
            let bytes = &buf[..(meta.bytesused as usize).min(buf.len())];
            let (w, h, rgba) = if self.mjpeg {
                let Ok(img) = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg) else { continue }; // a torn frame
                let img = img.to_rgba8();
                (img.width(), img.height(), img.into_raw())
            } else {
                (self.w, self.h, yuyv_to_rgba(bytes, self.w as usize, self.h as usize).context("short camera frame")?)
            };
            return Ok(Some(even(w, h, rgba)));
        }
    }
}

/// Packed 4:2:2 (Y0 U Y1 V) to RGBA, BT.601 limited range.
fn yuyv_to_rgba(yuyv: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let row = w * 2;
    if w % 2 != 0 || yuyv.len() < row * h {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for q in yuyv[..row * h].chunks_exact(4) {
        let (u, v) = (q[1] as i32 - 128, q[3] as i32 - 128);
        for y in [q[0], q[2]] {
            let c = 298 * (y as i32 - 16);
            let px = |x: i32| ((x + 128) >> 8).clamp(0, 255) as u8;
            out.extend_from_slice(&[px(c + 409 * v), px(c - 100 * u - 208 * v), px(c + 516 * u), 255]);
        }
    }
    Some(out)
}

/// The picture cropped to even width and height, as `Frame` requires.
fn even(w: u32, h: u32, rgba: Vec<u8>) -> Frame {
    let (ew, eh) = (w & !1, h & !1);
    let rgba = if (ew, eh) == (w, h) {
        rgba
    } else {
        rgba.chunks_exact(w as usize * 4).take(eh as usize).flat_map(|r| &r[..ew as usize * 4]).copied().collect()
    };
    Frame { width: ew, height: eh, rgba, at: Instant::now() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_yuyv_and_crops_odd_sizes() {
        // White, black, then a pure-red pair (BT.601: Y 81, U 90, V 240).
        let px = yuyv_to_rgba(&[235, 128, 235, 128, 16, 128, 16, 128, 81, 90, 81, 240], 6, 1).unwrap();
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
        assert_eq!(&px[8..12], &[0, 0, 0, 255]);
        assert!(px[16] > 250 && px[17] < 5 && px[18] < 5, "{:?}", &px[16..20]);
        assert!(yuyv_to_rgba(&[0; 8], 4, 2).is_none());
        let frame = even(3, 3, (0..36).collect());
        assert_eq!((frame.width, frame.height, frame.rgba.len()), (2, 2, 16));
        assert_eq!(&frame.rgba[8..12], &[12, 13, 14, 15]);
    }

    #[test]
    fn picks_the_widest_camera_size_that_fits() {
        use v4l::framesize::{Discrete, FrameSize, FrameSizeEnum};
        let sizes: Vec<FrameSize> = [(640, 480), (1920, 1080), (1280, 720)]
            .into_iter()
            .enumerate()
            .map(|(i, (width, height))| FrameSize { index: i as u32, fourcc: v4l::FourCC::new(MJPG), typ: 1, size: FrameSizeEnum::Discrete(Discrete { width, height }) })
            .collect();
        assert_eq!(best_size(&sizes, 1280), Some((1280, 720)));
        assert_eq!(best_size(&sizes, 320), Some((640, 480)));
        assert_eq!(best_size(&[], 1280), None);
    }
}
