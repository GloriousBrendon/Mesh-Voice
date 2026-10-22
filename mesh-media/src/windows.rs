//! Windows: screens and windows through Windows Graphics Capture (thumbnails through GDI, which
//! is quick for one small picture but ~70 % of a core at 15 fps on a 1440p screen), cameras
//! through Media Foundation.
// ponytail: frames are copied off the GPU and shrunk on the CPU (~5 ms at 1440p); a GPU scaler
// (D3D11 video processor) would save that if it ever matters.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, GUID, Interface as _, PWSTR};

use crate::{Camera, Frame, Grab, Kind, Opts, SourceInfo, fit};

const THUMB_WIDTH: u32 = 240;
/// Also draws windows that compose with DirectComposition (browsers, UWP apps).
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// Physical pixels on this thread while it lives, so a scaled (125 %, 150 %…) screen is
/// captured whole and at full resolution.
struct DpiAware(DPI_AWARENESS_CONTEXT);

impl DpiAware {
    fn new() -> DpiAware {
        DpiAware(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiAware {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

fn size(r: RECT) -> (u32, u32) {
    ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
}

// ---------- screens and windows ----------

struct Monitor {
    handle: HMONITOR,
    device: String,
    rect: RECT,
    primary: bool,
}

/// The screens, the primary one first.
fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
            list.push(Monitor { handle: monitor, device: wide(&info.szDevice), rect: info.monitorInfo.rcMonitor, primary: info.monitorInfo.dwFlags & 1 != 0 });
        }
        TRUE
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    list.sort_by_key(|m| !m.primary);
    list
}

/// Top-level windows worth sharing (visible, titled, not minimised, cloaked or a tool window),
/// front to back.
fn windows() -> Vec<(HWND, String)> {
    unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<(HWND, String)>) };
        if let Some(title) = shareable(hwnd) {
            list.push((hwnd, title));
        }
        TRUE
    }
    let mut list = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut Vec<(HWND, String)> as isize));
    }
    list
}

fn shareable(hwnd: HWND) -> Option<String> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4).is_ok() && cloaked != 0 {
            return None;
        }
        let mut title = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut title).max(0) as usize;
        let title = String::from_utf16_lossy(&title[..n]);
        let (w, h) = size(visible_rect(hwnd)?);
        (!title.trim().is_empty() && w > 1 && h > 1).then_some(title)
    }
}

/// A window's rectangle without its invisible resize borders.
fn visible_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32).is_ok() {
            return Some(r);
        }
        GetWindowRect(hwnd, &mut r).ok().map(|()| r)
    }
}

/// A memory DC drawing into pixels we can read.
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *const u8,
    w: i32,
    h: i32,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Result<Canvas> {
        let (w, h) = (w as i32, h as i32);
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                bail!("no memory DC");
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    bail!("no {w}x{h} bitmap");
                }
            };
            let old = SelectObject(dc, bitmap.into());
            SetStretchBltMode(dc, HALFTONE);
            let _ = SetBrushOrgEx(dc, 0, 0, None);
            Ok(Canvas { dc, bitmap, old, bits: bits as *const u8, w, h })
        }
    }

    /// `slot`'s canvas, made anew if it isn't `w`×`h`.
    fn sized(slot: &mut Option<Canvas>, w: u32, h: u32) -> Result<&Canvas> {
        if slot.as_ref().is_none_or(|c| (c.w, c.h) != (w as i32, h as i32)) {
            *slot = None;
            *slot = Some(Canvas::new(w, h)?);
        }
        Ok(slot.as_ref().expect("just made"))
    }

    /// What's drawn, as RGBA.
    fn frame(&self) -> Frame {
        unsafe {
            let _ = GdiFlush();
        }
        let len = (self.w * self.h * 4) as usize;
        let mut rgba = unsafe { std::slice::from_raw_parts(self.bits, len) }.to_vec();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Frame { width: self.w as u32, height: self.h as u32, rgba, at: Instant::now() }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Screen area `src` scaled onto all of `canvas`, with the mouse pointer.
fn draw_screen(canvas: &Canvas, src: RECT) -> Result<()> {
    let (sw, sh) = size(src);
    unsafe {
        let screen = GetDC(None);
        let ok = StretchBlt(canvas.dc, 0, 0, canvas.w, canvas.h, Some(screen), src.left, src.top, sw as i32, sh as i32, SRCCOPY).as_bool();
        ReleaseDC(None, screen);
        if !ok {
            bail!("couldn't copy the screen");
        }
        let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut info).is_err() || info.flags.0 & CURSOR_SHOWING.0 == 0 {
            return Ok(());
        }
        let icon = HICON(info.hCursor.0);
        let mut icon_info = ICONINFO::default();
        let hot = if GetIconInfo(icon, &mut icon_info).is_ok() {
            for b in [icon_info.hbmMask, icon_info.hbmColor] {
                if !b.is_invalid() {
                    let _ = DeleteObject(b.into());
                }
            }
            (icon_info.xHotspot as i32, icon_info.yHotspot as i32)
        } else {
            (0, 0)
        };
        let x = (info.ptScreenPos.x - src.left) * canvas.w / (sw as i32).max(1) - hot.0;
        let y = (info.ptScreenPos.y - src.top) * canvas.h / (sh as i32).max(1) - hot.1;
        let _ = DrawIconEx(canvas.dc, x, y, icon, 0, 0, 0, None, DI_NORMAL);
    }
    Ok(())
}

/// A window (even one behind others) scaled to `max_width` at most.
fn draw_window(hwnd: HWND, whole: &mut Option<Canvas>, out: &mut Option<Canvas>, max_width: u32) -> Result<Frame> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            bail!("the window closed");
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect)?;
        let vis = visible_rect(hwnd).unwrap_or(rect);
        let (fw, fh) = size(rect);
        let whole = Canvas::sized(whole, fw.max(2), fh.max(2))?;
        if !PrintWindow(hwnd, whole.dc, PW_RENDERFULLCONTENT).as_bool() {
            bail!("couldn't draw the window");
        }
        let (vw, vh) = size(vis);
        let (tw, th) = fit(vw, vh, max_width);
        let out = Canvas::sized(out, tw, th)?;
        let ok = StretchBlt(out.dc, 0, 0, tw as i32, th as i32, Some(whole.dc), vis.left - rect.left, vis.top - rect.top, vw as i32, vh as i32, SRCCOPY);
        if !ok.as_bool() {
            bail!("couldn't scale the window");
        }
        Ok(out.frame())
    }
}

enum Target {
    Screen(HMONITOR),
    Window(HWND),
}

/// `screen:<device>` or `window:<handle>`; `""` is the primary screen.
fn target(id: &str) -> Result<Target> {
    if let Some(hwnd) = id.strip_prefix("window:") {
        let hwnd = HWND(hwnd.parse::<usize>().context("bad window id")? as *mut c_void);
        if hwnd.is_invalid() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            bail!("that window has closed");
        }
        return Ok(Target::Window(hwnd));
    }
    let device = id.strip_prefix("screen:").unwrap_or(id);
    monitors()
        .into_iter()
        .find(|m| device.is_empty() || m.device == device)
        .map(|m| Target::Screen(m.handle))
        .ok_or_else(|| anyhow!("that screen isn't there"))
}

/// COM, multithreaded, for as long as this lives.
struct Com(bool);

impl Com {
    fn start() -> Com {
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

const PIXELS: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

/// A live screen or window through Windows Graphics Capture (Windows 10 1903+; Windows draws its
/// yellow "being shared" border around it). It only hands over a frame when something changed,
/// so a still picture is sent again to keep the video going.
struct Wgc {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    d3d: IDirect3DDevice,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    size: SizeInt32,
    window: Option<HWND>,
    last: Option<Frame>,
    max_width: u32,
    gap: Duration,
    next: Instant,
    _dpi: DpiAware,
    _com: Com,
}

impl Drop for Wgc {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

pub fn open_source(id: &str, opts: Opts) -> Result<Box<dyn Grab>> {
    let (com, dpi) = (Com::start(), DpiAware::new());
    let target = target(id)?;
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        bail!("screen capture needs Windows 10 (1903) or later");
    }
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))
            .context("no Direct3D device")?;
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.context("no Direct3D device")?, context.context("no Direct3D context")?);
        let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()?;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let (item, window): (GraphicsCaptureItem, _) = match target {
            Target::Screen(monitor) => (interop.CreateForMonitor(monitor)?, None),
            Target::Window(hwnd) => (interop.CreateForWindow(hwnd).context("that window can't be captured")?, Some(hwnd)),
        };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, PIXELS, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        session.StartCapture()?;
        let gap = Duration::from_secs(1) / opts.fps.max(1);
        Ok(Box::new(Wgc {
            session,
            pool,
            d3d,
            device,
            context,
            staging: None,
            size,
            window,
            last: None,
            max_width: opts.max_width,
            gap,
            next: Instant::now(),
            _dpi: dpi,
            _com: com,
        }))
    }
}

impl Grab for Wgc {
    fn grab(&mut self) -> Result<Option<Frame>> {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        self.next = self.next.max(now) + self.gap;
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        match newest {
            Some(frame) => {
                let size = frame.ContentSize()?;
                let read = self.read(&frame, size);
                let _ = frame.Close();
                if size != self.size {
                    // A resized window: frames come at the new size from now on.
                    self.size = size;
                    self.pool.Recreate(&self.d3d, PIXELS, 2, size)?;
                }
                self.last = Some(read?);
            }
            None if self.window.is_some_and(|w| !unsafe { IsWindow(Some(w)) }.as_bool()) => bail!("the window closed"),
            None => {}
        }
        Ok(self.last.as_ref().map(|f| Frame { at: Instant::now(), ..f.clone() }))
    }
}

impl Wgc {
    fn read(&mut self, frame: &Direct3D11CaptureFrame, content: SizeInt32) -> Result<Frame> {
        unsafe {
            let texture: ID3D11Texture2D = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if self.staging.as_ref().is_none_or(|(_, w, h)| (*w, *h) != (desc.Width, desc.Height)) {
                let mut staging_desc = desc;
                staging_desc.Usage = D3D11_USAGE_STAGING;
                staging_desc.BindFlags = 0;
                staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                staging_desc.MiscFlags = 0;
                let mut staging = None;
                self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
                self.staging = Some((staging.context("no staging texture")?, desc.Width, desc.Height));
            }
            let staging = &self.staging.as_ref().expect("just made").0;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            // The content can be smaller than the texture while a window is being resized.
            let w = (content.Width.max(0) as u32).min(desc.Width);
            let h = (content.Height.max(0) as u32).min(desc.Height);
            let (dw, dh) = fit(w, h, self.max_width);
            let pitch = mapped.RowPitch as usize;
            let bgra = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h as usize);
            let rgba = crate::shrink(bgra, pitch, (w as usize, h as usize), (dw as usize, dh as usize));
            self.context.Unmap(staging, 0);
            Ok(Frame { width: dw, height: dh, rgba, at: Instant::now() })
        }
    }
}

pub fn list_sources() -> Vec<SourceInfo> {
    let _dpi = DpiAware::new();
    let thumb = |frame: Result<Frame>| frame.and_then(|f| f.jpeg(THUMB_WIDTH, 70)).unwrap_or_default();
    let mut list: Vec<SourceInfo> = monitors()
        .into_iter()
        .enumerate()
        .map(|(n, m)| {
            let (w, h) = size(m.rect);
            let (tw, th) = fit(w, h, THUMB_WIDTH);
            let mut slot = None;
            let frame = Canvas::sized(&mut slot, tw, th).and_then(|c| draw_screen(c, m.rect).map(|()| c.frame()));
            SourceInfo { id: format!("screen:{}", m.device), name: format!("Screen {} ({w}×{h})", n + 1), kind: Kind::Screen, thumbnail_jpeg: thumb(frame) }
        })
        .collect();
    for (hwnd, title) in windows() {
        let frame = draw_window(hwnd, &mut None, &mut None, THUMB_WIDTH);
        list.push(SourceInfo { id: format!("window:{}", hwnd.0 as usize), name: title, kind: Kind::Window, thumbnail_jpeg: thumb(frame) });
    }
    list
}

// ---------- cameras ----------

/// Media Foundation (and COM) for as long as this lives.
struct Mf {
    started: bool,
    _com: Com,
}

impl Mf {
    fn start() -> Result<Mf> {
        let mut mf = Mf { started: false, _com: Com::start() };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.context("Media Foundation didn't start")?;
        mf.started = true;
        Ok(mf)
    }
}

impl Drop for Mf {
    fn drop(&mut self) {
        if self.started {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }
}

fn devices() -> Result<Vec<IMFActivate>> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.context("no attributes")?;
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let (mut list, mut n) = (null_mut(), 0u32);
        MFEnumDeviceSources(&attrs, &mut list, &mut n)?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let found = (0..n as usize).filter_map(|i| (*list.add(i)).take()).collect();
        CoTaskMemFree(Some(list as *const c_void));
        Ok(found)
    }
}

fn text(device: &IMFActivate, key: &GUID) -> String {
    unsafe {
        let (mut s, mut n) = (PWSTR::null(), 0u32);
        if device.GetAllocatedString(key, &mut s, &mut n).is_err() {
            return String::new();
        }
        let t = s.to_string().unwrap_or_default();
        CoTaskMemFree(Some(s.0 as *const c_void));
        t
    }
}

fn link(device: &IMFActivate) -> String {
    text(device, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)
}

pub fn list_cameras() -> Vec<Camera> {
    let Ok(_mf) = Mf::start() else { return Vec::new() };
    devices()
        .unwrap_or_default()
        .iter()
        .map(|d| Camera { id: link(d), name: text(d, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME) })
        .collect()
}

/// A packed (high, low) pair, as frame sizes and rates are.
fn pair(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// The camera mode to ask for: the widest that fits `max_width` at `fps` or better.
fn best_mode(reader: &IMFSourceReader, opts: Opts) -> Option<IMFMediaType> {
    let mut best: Option<((bool, bool, i64, u32), IMFMediaType)> = None;
    for i in 0.. {
        let Ok(mode) = (unsafe { reader.GetNativeMediaType(STREAM, i) }) else { break };
        let Ok((w, _)) = unsafe { mode.GetUINT64(&MF_MT_FRAME_SIZE) }.map(pair) else { continue };
        let fps = unsafe { mode.GetUINT64(&MF_MT_FRAME_RATE) }.map(pair).map_or(0, |(n, d)| n / d.max(1));
        let fits = w <= opts.max_width;
        let key = (fits, fps >= opts.fps, if fits { w as i64 } else { -(w as i64) }, fps);
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, mode));
        }
    }
    best.map(|(_, mode)| mode)
}

struct Cam {
    reader: IMFSourceReader,
    source: IMFMediaSource,
    w: u32,
    h: u32,
    /// Row pitch of a plain (not 2-D) buffer; negative when bottom-up.
    stride: i32,
    gap: Duration,
    last: Option<Instant>,
    _mf: Mf,
}

impl Drop for Cam {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.Shutdown();
        }
    }
}

pub fn open_camera(id: &str, opts: Opts) -> Result<Box<dyn Grab>> {
    let mf = Mf::start()?;
    let device = devices()?
        .into_iter()
        .find(|d| id.is_empty() || link(d) == id)
        .ok_or_else(|| anyhow!(if id.is_empty() { "no camera found" } else { "that camera isn't there" }))?;
    unsafe {
        let source: IMFMediaSource = device.ActivateObject().context("couldn't open the camera (in use, or blocked in Windows privacy settings?)")?;
        let opened = (|| -> Result<(IMFSourceReader, u32, u32, i32)> {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.context("no attributes")?;
            // Lets the reader convert whatever the camera sends (MJPG, YUY2, NV12) to RGB32.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
            if let Some(mode) = best_mode(&reader, opts) {
                reader.SetCurrentMediaType(STREAM, None, &mode)?;
            }
            let rgb = MFCreateMediaType()?;
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(STREAM, None, &rgb).context("the camera can't give RGB")?;
            let current = reader.GetCurrentMediaType(STREAM)?;
            let (w, h) = pair(current.GetUINT64(&MF_MT_FRAME_SIZE)?);
            let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or(w as i32 * 4, |s| s as i32);
            Ok((reader, w, h, stride))
        })();
        let (reader, w, h, stride) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                let _ = source.Shutdown();
                return Err(e);
            }
        };
        let gap = Duration::from_secs(1) / opts.fps.max(1);
        Ok(Box::new(Cam { reader, source, w, h, stride, gap, last: None, _mf: mf }))
    }
}

impl Grab for Cam {
    fn grab(&mut self) -> Result<Option<Frame>> {
        loop {
            let (mut flags, mut sample) = (0u32, None);
            unsafe { self.reader.ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample)) }.context("the camera stopped")?;
            if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
                bail!("the camera stopped");
            }
            let Some(sample) = sample else { continue };
            // A 30 fps camera at 15: take every other frame, not every third.
            if self.last.is_some_and(|t| t.elapsed() < self.gap * 3 / 4) {
                continue;
            }
            self.last = Some(Instant::now());
            return unsafe { self.picture(&sample) }.map(Some);
        }
    }
}

impl Cam {
    unsafe fn picture(&self, sample: &IMFSample) -> Result<Frame> {
        let (w, h) = (self.w as usize, self.h as usize);
        let (ew, eh) = (w & !1, h & !1);
        let mut rgba = Vec::with_capacity(ew * eh * 4);
        unsafe {
            let buffer = sample.GetBufferByIndex(0)?;
            if let Ok(buffer) = buffer.cast::<IMF2DBuffer>() {
                // The first row whichever way up the buffer is, and the step to the next.
                let (mut row0, mut pitch) = (null_mut(), 0i32);
                buffer.Lock2D(&mut row0, &mut pitch)?;
                for y in 0..eh {
                    rgba.extend_from_slice(std::slice::from_raw_parts(row0.offset(y as isize * pitch as isize), ew * 4));
                }
                let _ = buffer.Unlock2D();
            } else {
                let (mut data, mut len) = (null_mut(), 0u32);
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let row = self.stride.unsigned_abs() as usize;
                if row < w * 4 || (len as usize) < row * h {
                    let _ = buffer.Unlock();
                    bail!("short camera frame");
                }
                for y in 0..eh {
                    let line = if self.stride < 0 { h - 1 - y } else { y };
                    rgba.extend_from_slice(std::slice::from_raw_parts(data.add(line * row), ew * 4));
                }
                let _ = buffer.Unlock();
            }
        }
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(Frame { width: ew as u32, height: eh as u32, rgba, at: Instant::now() })
    }
}
