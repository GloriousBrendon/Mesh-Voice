//! cpal mic/speaker for Discord voice. The mic is a blocking `MediaSource` of
//! mono f32 PCM that songbird wraps in a `RawAdapter` (it resamples to 48 kHz
//! itself); the speaker is fed the mono i16 songbird decodes per 20 ms tick.
//! Same cpal shape as `mesh-wa/src/audio.rs` (MIT, (c) João Lucas); kept
//! separate because the sample formats differ.

use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{I24, Sample, SampleFormat, U24};
use log::error;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer as _, Producer as _, Split as _};
use songbird::input::core::io::MediaSource;

/// A live microphone as a blocking reader of mono f32 little-endian samples.
/// The consumer sits behind a mutex so the whole thing is `Sync`, which
/// songbird's `MediaSource` bound requires (only its own input thread reads it).
pub struct Mic {
    cons: std::sync::Mutex<ringbuf::HeapCons<f32>>,
    alive: Arc<AtomicBool>,
    pub rate: u32,
    scratch: Vec<u8>,
}

/// Opens the default mic. Returns the reader and its native sample rate (for
/// `RawAdapter::new(mic, rate, 1)`).
pub fn open_mic() -> Result<Mic> {
    let device = cpal::default_host().default_input_device().ok_or_else(|| anyhow!("no microphone"))?;
    let supported = device.default_input_config().map_err(|e| anyhow!("mic config: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate;
    let channels = config.channels as usize;
    let (prod, cons) = HeapRb::<f32>::new(rate as usize).split();
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        std::thread::spawn(move || {
            match build_input(&device, &config, format, channels, prod) {
                Ok(stream) => match stream.play() {
                    Ok(()) => while alive.load(Ordering::Relaxed) {
                        std::thread::park_timeout(Duration::from_millis(250));
                    },
                    Err(e) => error!("mic play: {e}"),
                },
                Err(e) => error!("mic build: {e}"),
            }
            alive.store(false, Ordering::Relaxed);
        });
    }
    Ok(Mic { cons: std::sync::Mutex::new(cons), alive, rate, scratch: Vec::new() })
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

impl Read for Mic {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        // Block (briefly) until there's at least one sample, so songbird's
        // blocking input thread never busy-spins on an empty mic.
        let want = buf.len() / 4;
        if want == 0 {
            return Ok(0);
        }
        self.scratch.clear();
        let mut popped = vec![0f32; want];
        let mut n = 0;
        for _ in 0..200 {
            n = self.cons.lock().unwrap().pop_slice(&mut popped);
            if n > 0 || !self.alive.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if n == 0 {
            // Feed silence rather than EOF, so the track stays alive.
            popped.iter_mut().for_each(|s| *s = 0.0);
            n = want.min(480);
        }
        for s in &popped[..n] {
            self.scratch.extend_from_slice(&s.to_le_bytes());
        }
        let len = self.scratch.len().min(buf.len());
        buf[..len].copy_from_slice(&self.scratch[..len]);
        Ok(len)
    }
}

impl Seek for Mic {
    fn seek(&mut self, _: SeekFrom) -> IoResult<u64> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

impl MediaSource for Mic {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn build_input<P>(device: &cpal::Device, config: &cpal::StreamConfig, format: SampleFormat, channels: usize, mut prod: P) -> Result<cpal::Stream>
where
    P: ringbuf::traits::Producer<Item = f32> + Send + 'static,
{
    macro_rules! build {
        ($t:ty) => {{
            let mut mono: Vec<f32> = Vec::with_capacity(2048);
            device
                .build_input_stream(
                    config.clone(),
                    move |data: &[$t], _| {
                        mono.clear();
                        for frame in data.chunks(channels) {
                            let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                            mono.push(sum / channels as f32);
                        }
                        let _ = prod.push_slice(&mono);
                    },
                    |e| error!("mic stream error: {e}"),
                    None,
                )
                .map_err(|e| anyhow!("build input: {e}"))
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
        other => Err(anyhow!("unsupported input format {other:?}")),
    }
}

const VOICE_RATE: u32 = 48_000;

/// The default speaker, fed 48 kHz mono i16 frames.
pub fn open_speaker() -> Result<async_channel::Sender<Vec<i16>>> {
    let device = cpal::default_host().default_output_device().ok_or_else(|| anyhow!("no speaker"))?;
    let supported = device.default_output_config().map_err(|e| anyhow!("speaker config: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let dst_rate = config.sample_rate;
    let channels = config.channels as usize;
    let (mut prod, cons) = HeapRb::<i16>::new(dst_rate as usize).split();
    let _ = prod.push_slice(&vec![0i16; dst_rate as usize / 10]); // 100 ms pre-roll

    let (tx, rx) = async_channel::bounded::<Vec<i16>>(64);
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = done.clone();
        tokio::spawn(async move {
            let mut pos = 0.0f64;
            let mut out: Vec<i16> = Vec::with_capacity(4096);
            while let Ok(frame) = rx.recv().await {
                out.clear();
                resample(&frame, VOICE_RATE, dst_rate, &mut pos, &mut out);
                let _ = prod.push_slice(&out);
            }
            done.store(true, Ordering::Relaxed);
        });
    }
    std::thread::spawn(move || {
        let stream = match build_output(&device, &config, format, channels, cons) {
            Ok(s) => s,
            Err(e) => return error!("speaker build: {e}"),
        };
        if let Err(e) = stream.play() {
            return error!("speaker play: {e}");
        }
        while !done.load(Ordering::Relaxed) {
            std::thread::park_timeout(Duration::from_millis(250));
        }
    });
    Ok(tx)
}

/// Linear resample carrying the fractional cursor across blocks.
fn resample(src: &[i16], src_rate: u32, dst_rate: u32, pos: &mut f64, out: &mut Vec<i16>) {
    if src.is_empty() || src_rate == dst_rate {
        out.extend_from_slice(src);
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

fn build_output<C>(device: &cpal::Device, config: &cpal::StreamConfig, format: SampleFormat, channels: usize, mut cons: C) -> Result<cpal::Stream>
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
                .map_err(|e| anyhow!("build output: {e}"))
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
        other => Err(anyhow!("unsupported output format {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_and_doubles() {
        let mut pos = 0.0;
        let mut out = Vec::new();
        resample(&[0, 100, 200, 300], 48_000, 24_000, &mut pos, &mut out);
        assert_eq!(out, vec![0, 200]); // every other sample
        out.clear();
        pos = 0.0;
        resample(&[0, 0, 0], 48_000, 48_000, &mut pos, &mut out);
        assert_eq!(out.len(), 3); // pass-through at equal rate
    }
}
