//! cpal mic/speaker bridge for calls: 16 kHz mono i16 in 960-sample (60 ms) frames.
//! Adapted from whatsapp-rust's examples/voip-cli (MIT, (c) João Lucas).
//!
//! cpal's `Stream` is `!Send` on some hosts, so each stream lives on its own std thread that
//! drops it (freeing the device) once the call drops its end of the channel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{I24, Sample, SampleFormat, U24};
use log::{error, warn};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer as _, Producer as _, Split as _};

const FRAME_SAMPLES: usize = 960;
const WA_RATE: u32 = 16_000;

/// Linear resample `src` into `out`, carrying the fractional cursor across blocks.
fn resample_into(src: &[i16], src_rate: u32, dst_rate: u32, pos: &mut f64, out: &mut Vec<i16>) {
    if src.is_empty() {
        return;
    }
    let step = src_rate as f64 / dst_rate as f64;
    let mut p = *pos;
    while p < src.len() as f64 {
        let i = p as usize;
        let frac = p - i as f64;
        let a = src[i] as f64;
        let b = if i + 1 < src.len() { src[i + 1] as f64 } else { a };
        out.push((a + (b - a) * frac).round() as i16);
        p += step;
    }
    *pos = p - src.len() as f64;
}

fn default_config(device: &cpal::Device, input: bool) -> Result<(cpal::StreamConfig, SampleFormat)> {
    let supported = if input { device.default_input_config() } else { device.default_output_config() }
        .map_err(|e| anyhow!("default audio config: {e}"))?;
    let format = supported.sample_format();
    Ok((supported.into(), format))
}

/// The default microphone as 960-sample 16 kHz frames.
pub fn spawn_mic() -> Result<async_channel::Receiver<Vec<i16>>> {
    let device = cpal::default_host().default_input_device().ok_or_else(|| anyhow!("no microphone"))?;
    let (config, format) = default_config(&device, true)?;
    let src_rate = config.sample_rate;
    let channels = config.channels as usize;
    let (tx, rx) = async_channel::bounded::<Vec<i16>>(3);
    let (prod, mut cons) = HeapRb::<i16>::new(src_rate as usize).split();
    let peak = Arc::new(AtomicI32::new(0));
    let alive = Arc::new(AtomicBool::new(true));

    // Drain: ring -> 16 kHz -> 960-sample frames -> channel. Never touches the realtime callback.
    {
        let (peak, alive, tx) = (peak.clone(), alive.clone(), tx.clone());
        tokio::spawn(async move {
            let mut pop_buf = vec![0i16; (src_rate as usize / 4).max(FRAME_SAMPLES)];
            let mut acc: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 2);
            let mut pos = 0.0f64;
            while alive.load(Ordering::Relaxed) && !tx.is_closed() {
                let n = cons.pop_slice(&mut pop_buf);
                if n == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                }
                resample_into(&pop_buf[..n], src_rate, WA_RATE, &mut pos, &mut acc);
                while acc.len() >= FRAME_SAMPLES {
                    let rest = acc.split_off(FRAME_SAMPLES);
                    let frame = std::mem::replace(&mut acc, rest);
                    peak.fetch_max(frame.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0), Ordering::Relaxed);
                    let _ = tx.try_send(frame); // drop on full: loss tolerant
                }
            }
        });
    }
    {
        let (peak, alive) = (peak.clone(), alive.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if alive.load(Ordering::Relaxed) && peak.load(Ordering::Relaxed) == 0 {
                warn!("microphone is capturing only silence (Windows: Settings > Privacy & security > Microphone)");
            }
        });
    }
    std::thread::spawn(move || {
        match build_input_stream(&device, &config, format, channels, prod) {
            Ok(stream) => match stream.play() {
                Ok(()) => {
                    while !tx.is_closed() && alive.load(Ordering::Relaxed) {
                        std::thread::park_timeout(Duration::from_millis(250));
                    }
                }
                Err(e) => error!("mic stream play failed: {e}"),
            },
            Err(e) => error!("mic stream build failed: {e}"),
        }
        alive.store(false, Ordering::Relaxed);
    });
    Ok(rx)
}

fn build_input_stream<P>(device: &cpal::Device, config: &cpal::StreamConfig, format: SampleFormat, channels: usize, mut prod: P) -> Result<cpal::Stream>
where
    P: ringbuf::traits::Producer<Item = i16> + Send + 'static,
{
    macro_rules! build {
        ($t:ty) => {{
            let mut mono: Vec<i16> = Vec::with_capacity(2048);
            device
                .build_input_stream(
                    config.clone(),
                    move |data: &[$t], _| {
                        mono.clear();
                        for frame in data.chunks(channels) {
                            let sum: i32 = frame.iter().map(|&s| i16::from_sample(s) as i32).sum();
                            mono.push((sum / channels as i32) as i16);
                        }
                        let _ = prod.push_slice(&mono);
                    },
                    |e| error!("mic stream error: {e}"),
                    None,
                )
                .map_err(|e| anyhow!("build input stream: {e}"))
        }};
    }
    match format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::I24 => build!(I24),
        SampleFormat::U24 => build!(U24),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => Err(anyhow!("unsupported input sample format {other:?}")),
    }
}

/// The default speaker, fed 16 kHz mono frames.
pub fn spawn_speaker() -> Result<async_channel::Sender<Vec<i16>>> {
    let device = cpal::default_host().default_output_device().ok_or_else(|| anyhow!("no speaker"))?;
    let (config, format) = default_config(&device, false)?;
    let dst_rate = config.sample_rate;
    let channels = config.channels as usize;
    let (mut prod, cons) = HeapRb::<i16>::new(dst_rate as usize).split();
    // WASAPI pulls every ~10 ms while frames come in 60 ms bursts: pre-roll 100 ms of silence.
    let _ = prod.push_slice(&vec![0i16; dst_rate as usize / 10]);

    let (tx, rx) = async_channel::bounded::<Vec<i16>>(64);
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = done.clone();
        tokio::spawn(async move {
            let mut resampled: Vec<i16> = Vec::with_capacity(4096);
            let mut pos = 0.0f64;
            while let Ok(frame) = rx.recv().await {
                resampled.clear();
                resample_into(&frame, WA_RATE, dst_rate, &mut pos, &mut resampled);
                let _ = prod.push_slice(&resampled);
            }
            done.store(true, Ordering::Relaxed);
        });
    }
    std::thread::spawn(move || {
        let stream = match build_output_stream(&device, &config, format, channels, cons) {
            Ok(s) => s,
            Err(e) => return error!("speaker stream build failed: {e}"),
        };
        if let Err(e) = stream.play() {
            return error!("speaker stream play failed: {e}");
        }
        while !done.load(Ordering::Relaxed) {
            std::thread::park_timeout(Duration::from_millis(250));
        }
    });
    Ok(tx)
}

fn build_output_stream<C>(device: &cpal::Device, config: &cpal::StreamConfig, format: SampleFormat, channels: usize, mut cons: C) -> Result<cpal::Stream>
where
    C: ringbuf::traits::Consumer<Item = i16> + Send + 'static,
{
    macro_rules! build {
        ($t:ty) => {{
            device
                .build_output_stream(
                    config.clone(),
                    move |data: &mut [$t], _| {
                        for frame in data.chunks_mut(channels) {
                            let v = <$t>::from_sample(cons.try_pop().unwrap_or(0));
                            frame.fill(v);
                        }
                    },
                    |e| error!("speaker stream error: {e}"),
                    None,
                )
                .map_err(|e| anyhow!("build output stream: {e}"))
        }};
    }
    match format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::I24 => build!(I24),
        SampleFormat::U24 => build!(U24),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => Err(anyhow!("unsupported output sample format {other:?}")),
    }
}
