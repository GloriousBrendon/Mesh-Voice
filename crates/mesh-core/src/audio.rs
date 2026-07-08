//! Audio capture and playback for 1:1 calls.
//!
//! Pure-Rust: [`cpal`] for the mic/speaker devices and [`opus`] for the codec.
//! WebRTC media is Opus at 48 kHz; we capture/playback mono in 20 ms frames
//! (960 samples), which is what the peer connection feeds to and reads from the
//! RTP tracks in [`crate::call`].
//!
//! `cpal::Stream` is `!Send`, so each stream lives on its own dedicated thread
//! and talks to the async world through channels. Dropping the returned handle
//! stops the thread and releases the device.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

/// Opus/WebRTC sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one 20 ms mono frame.
pub const FRAME_SAMPLES: usize = 960;

/// Keeps the capture thread (and its `cpal` input stream) alive. Drop to stop.
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Keeps the playback thread (and its `cpal` output stream) alive. Drop to stop.
pub struct PlaybackHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Names of available input (microphone) devices.
pub fn input_device_names() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|it| {
            it.filter_map(|d| d.description().ok().map(|desc| desc.name().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Names of available output (speaker) devices.
pub fn output_device_names() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|it| {
            it.filter_map(|d| d.description().ok().map(|desc| desc.name().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Picks an input device by name, falling back to the system default.
fn pick_input(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(name) = name
        && let Ok(mut devices) = host.input_devices()
        && let Some(device) =
            devices.find(|d| d.description().map(|desc| desc.name() == name).unwrap_or(false))
    {
        return Some(device);
    }
    host.default_input_device()
}

/// Picks an output device by name, falling back to the system default.
fn pick_output(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(name) = name
        && let Ok(mut devices) = host.output_devices()
        && let Some(device) =
            devices.find(|d| d.description().map(|desc| desc.name() == name).unwrap_or(false))
    {
        return Some(device);
    }
    host.default_output_device()
}

/// Starts capturing the chosen input device (or the system default when
/// `device_name` is `None`), delivering 20 ms mono `i16` frames on the
/// returned channel. Resamples/​downmixes the device's native config to
/// 48 kHz mono so callers always get [`FRAME_SAMPLES`]-sized frames.
pub fn start_capture(
    device_name: Option<String>,
) -> Result<(CaptureHandle, mpsc::UnboundedReceiver<Vec<i16>>), String> {
    let (tx, rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    // Build and own the stream on a dedicated thread: cpal streams are !Send.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread = std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = pick_input(&host, device_name.as_deref()) else {
            let _ = ready_tx.send(Err("no input (microphone) device".into()));
            return;
        };
        let default_config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("input config: {e}")));
                return;
            }
        };
        let device_rate = default_config.sample_rate();
        let device_channels = default_config.channels() as usize;
        let config: cpal::StreamConfig = default_config.clone().into();

        // Accumulates resampled mono samples until we have a full 20 ms frame.
        let mut acc: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 2);
        let mut resampler = Resampler::new(device_rate, SAMPLE_RATE);
        let tx_cb = tx.clone();
        let mut push_mono = move |mono: &[i16]| {
            for &s in mono {
                acc.push(s);
                if acc.len() == FRAME_SAMPLES {
                    let _ = tx_cb.send(std::mem::replace(
                        &mut acc,
                        Vec::with_capacity(FRAME_SAMPLES),
                    ));
                }
            }
        };

        let sample_format = default_config.sample_format();
        if !matches!(
            sample_format,
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16
        ) {
            let _ = ready_tx.send(Err(format!(
                "unsupported input sample format {sample_format:?}"
            )));
            return;
        }
        let err_fn = |e| tracing::warn!("capture stream error: {e}");
        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config.clone(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_f32(data, device_channels);
                    let up = resampler.process(&mono);
                    push_mono(&up);
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config.clone(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_i16(data, device_channels);
                    let up = resampler.process(&mono);
                    push_mono(&up);
                },
                err_fn,
                None,
            ),
            _ => unreachable!(),
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("build input stream: {e}")));
                return;
            }
        };
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(format!("play input stream: {e}")));
            return;
        }
        let _ = ready_tx.send(Ok(()));

        while !stop_thread.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(stream);
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok((
            CaptureHandle {
                stop,
                thread: Some(thread),
            },
            rx,
        )),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("capture thread died during setup".into()),
    }
}

/// Starts playback on the chosen output device (or the system default when
/// `device_name` is `None`). Push decoded 48 kHz mono `i16` samples into the
/// returned shared buffer; the output callback drains it.
pub fn start_playback(
    device_name: Option<String>,
) -> Result<(PlaybackHandle, Arc<Mutex<VecDeque<i16>>>), String> {
    let buffer: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
    let buffer_thread = buffer.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread = std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = pick_output(&host, device_name.as_deref()) else {
            let _ = ready_tx.send(Err("no output (speaker) device".into()));
            return;
        };
        let default_config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("output config: {e}")));
                return;
            }
        };
        let out_rate = default_config.sample_rate();
        let out_channels = default_config.channels() as usize;
        let config: cpal::StreamConfig = default_config.clone().into();
        // Resample decoded 48 kHz audio to the device rate on the way out.
        let resampler = Arc::new(Mutex::new(Resampler::new(SAMPLE_RATE, out_rate)));
        let pending: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));

        let err_fn = |e| tracing::warn!("playback stream error: {e}");
        let fill = {
            let buffer_cb = buffer_thread.clone();
            let resampler = resampler.clone();
            let pending = pending.clone();
            move |out_len: usize| -> Vec<i16> {
                let frames = out_len / out_channels.max(1);
                let mut pending = pending.lock().unwrap();
                while pending.len() < frames {
                    let chunk: Vec<i16> = {
                        let mut buf = buffer_cb.lock().unwrap();
                        let take = buf.len().min(FRAME_SAMPLES);
                        buf.drain(..take).collect()
                    };
                    if chunk.is_empty() {
                        break;
                    }
                    let res = resampler.lock().unwrap().process(&chunk);
                    pending.extend(res);
                }
                (0..frames)
                    .map(|_| pending.pop_front().unwrap_or(0))
                    .collect()
            }
        };

        let sample_format = default_config.sample_format();
        if !matches!(
            sample_format,
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16
        ) {
            let _ = ready_tx.send(Err(format!(
                "unsupported output sample format {sample_format:?}"
            )));
            return;
        }
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let fill = fill.clone();
                device.build_output_stream(
                    config.clone(),
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mono = fill(out.len());
                        for (i, frame) in out.chunks_mut(out_channels.max(1)).enumerate() {
                            let s = mono.get(i).copied().unwrap_or(0) as f32 / 32768.0;
                            for slot in frame.iter_mut() {
                                *slot = s;
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => device.build_output_stream(
                config.clone(),
                move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mono = fill(out.len());
                    for (i, frame) in out.chunks_mut(out_channels.max(1)).enumerate() {
                        let s = mono.get(i).copied().unwrap_or(0);
                        for slot in frame.iter_mut() {
                            *slot = s;
                        }
                    }
                },
                err_fn,
                None,
            ),
            _ => unreachable!(),
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("build output stream: {e}")));
                return;
            }
        };
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(format!("play output stream: {e}")));
            return;
        }
        let _ = ready_tx.send(Ok(()));

        while !stop_thread.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(stream);
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok((
            PlaybackHandle {
                stop,
                thread: Some(thread),
            },
            buffer,
        )),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("playback thread died during setup".into()),
    }
}

fn downmix_f32(data: &[f32], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return data.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let avg = frame.iter().sum::<f32>() / frame.len() as f32;
            (avg.clamp(-1.0, 1.0) * 32767.0) as i16
        })
        .collect()
}

fn downmix_i16(data: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}

/// Minimal linear resampler for mono `i16`. Good enough for voice; keeps the
/// dependency tree pure-Rust. When rates match it is a no-op passthrough.
struct Resampler {
    from: u32,
    to: u32,
    pos: f64,
    last: i16,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self { from, to, pos: 0.0, last: 0 }
    }

    fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.from == self.to || input.is_empty() {
            return input.to_vec();
        }
        let ratio = self.from as f64 / self.to as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / ratio) as usize + 1);
        while self.pos < input.len() as f64 {
            let idx = self.pos.floor() as usize;
            let frac = self.pos - idx as f64;
            let a = if idx == 0 { self.last } else { input[idx - 1] };
            let b = input.get(idx).copied().unwrap_or(a);
            out.push((a as f64 + (b as f64 - a as f64) * frac) as i16);
            self.pos += ratio;
        }
        self.pos -= input.len() as f64;
        self.last = *input.last().unwrap();
        out
    }
}
