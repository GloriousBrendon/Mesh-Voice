//! Mesh's sounds: a chirp for new messages and a ring for calls, each different per
//! network so you can tell where it's from, plus TeamSpeak-style voice channel cues.
//! Built in (synthesized here, so there's no audio file to license) or a file the
//! user picks: WAV, MP3, OGG or FLAC.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mesh_core::{Network, SAMPLE_RATE};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// A message worth a notification.
    Message(Network),
    /// A call ringing us.
    Ring(Network),
    /// Our call ringing the other side.
    Ringback,
    /// Someone joined / left the voice channel we're in.
    VoiceJoin,
    VoiceLeave,
}

impl Sound {
    pub const ALL: [Sound; 9] = [
        Sound::Message(Network::Matrix),
        Sound::Message(Network::Discord),
        Sound::Message(Network::WhatsApp),
        Sound::Ring(Network::Matrix),
        Sound::Ring(Network::Discord),
        Sound::Ring(Network::WhatsApp),
        Sound::Ringback,
        Sound::VoiceJoin,
        Sound::VoiceLeave,
    ];

    /// Its key under `[sounds.files]` in config.toml.
    pub fn key(self) -> String {
        match self {
            Sound::Message(n) => format!("message_{}", n.tag()),
            Sound::Ring(n) => format!("ring_{}", n.tag()),
            Sound::Ringback => "ringback".into(),
            Sound::VoiceJoin => "voice_join".into(),
            Sound::VoiceLeave => "voice_leave".into(),
        }
    }

    pub fn label(self) -> String {
        match self {
            Sound::Message(n) => format!("Message · {}", n.spec().name),
            Sound::Ring(n) => format!("Incoming call · {}", n.spec().name),
            Sound::Ringback => "Outgoing call ringing".into(),
            Sound::VoiceJoin => "Someone joined voice".into(),
            Sound::VoiceLeave => "Someone left voice".into(),
        }
    }

    fn repeats(self) -> bool {
        matches!(self, Sound::Ring(_) | Sound::Ringback)
    }
}

/// `[sounds]` in config.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// 0..=1.
    pub volume: f32,
    /// Sound key -> the user's own file for it.
    pub files: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true, volume: 0.7, files: BTreeMap::new() }
    }
}

/// Plays a sound once, off the UI thread.
pub fn play(sound: Sound, settings: &Settings, device: Option<String>) {
    if settings.enabled {
        spawn(sound, settings, device, None);
    }
}

/// A ring that loops until this is dropped.
pub struct Ringer(Arc<AtomicBool>);

impl Drop for Ringer {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub fn ring(sound: Sound, settings: &Settings, device: Option<String>) -> Ringer {
    let stop = Arc::new(AtomicBool::new(false));
    spawn(sound, settings, device, Some(stop.clone()));
    Ringer(stop)
}

fn spawn(sound: Sound, settings: &Settings, device: Option<String>, stop: Option<Arc<AtomicBool>>) {
    let file = settings.files.get(&sound.key()).cloned();
    let volume = settings.volume.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        let custom = file.as_deref().and_then(|f| decode(f).map_err(|e| tracing::warn!("sound {f}: {e}")).ok());
        // A file has no pause of its own between rings; give it one.
        let gap = if custom.is_some() { 1500 } else { 0 };
        let pcm: Vec<i16> = custom.unwrap_or_else(|| builtin(sound)).iter().map(|&s| (s as f32 * volume) as i16).collect();
        let (_stream, left) = match mesh_core::start_playback(device) {
            Ok(out) => out,
            Err(err) => return tracing::warn!("sound: {err}"),
        };
        let stopped = || stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
        loop {
            if let Ok(mut left) = left.lock() {
                left.extend(&pcm);
            }
            while left.lock().is_ok_and(|l| !l.is_empty()) {
                if stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !sound.repeats() || stop.is_none() {
                break;
            }
            for _ in 0..gap / 20 {
                if stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if stopped() {
                return;
            }
        }
        // Let the device play out what it already took.
        std::thread::sleep(Duration::from_millis(150));
    });
}

/// Mesh's own sounds: short, PDA-style square-ish tones.
fn builtin(sound: Sound) -> Vec<i16> {
    let mut out = Vec::new();
    match sound {
        Sound::Message(n) => {
            let f = match n {
                Network::Matrix => 1046.5,
                Network::Discord => 880.0,
                Network::WhatsApp => 1318.5,
            };
            tone(&mut out, &[f], 70, 0.35);
            silence(&mut out, 35);
            tone(&mut out, &[f * 1.5], 110, 0.35);
        }
        // Each network rings its own way.
        Sound::Ring(Network::Matrix) => {
            tone(&mut out, &[440.0, 480.0], 1000, 0.3);
            silence(&mut out, 2000);
        }
        Sound::Ring(Network::Discord) => {
            for _ in 0..10 {
                tone(&mut out, &[784.0], 50, 0.3);
                tone(&mut out, &[988.0], 50, 0.3);
            }
            silence(&mut out, 1600);
        }
        Sound::Ring(Network::WhatsApp) => {
            for _ in 0..2 {
                for f in [523.3, 659.3, 784.0, 1046.5] {
                    tone(&mut out, &[f], 120, 0.3);
                    silence(&mut out, 20);
                }
            }
            silence(&mut out, 1500);
        }
        Sound::Ringback => {
            tone(&mut out, &[425.0], 1000, 0.18);
            silence(&mut out, 3000);
        }
        Sound::VoiceJoin => {
            tone(&mut out, &[660.0], 60, 0.3);
            tone(&mut out, &[990.0], 90, 0.3);
        }
        Sound::VoiceLeave => {
            tone(&mut out, &[990.0], 60, 0.3);
            tone(&mut out, &[660.0], 90, 0.3);
        }
    }
    out
}

/// Sine tones with a little third harmonic (the retro edge), faded in and out.
fn tone(out: &mut Vec<i16>, freqs: &[f32], ms: u32, amp: f32) {
    let rate = SAMPLE_RATE as f32;
    let n = SAMPLE_RATE as usize * ms as usize / 1000;
    let fade = (rate * 0.005) as usize;
    for i in 0..n {
        let t = i as f32 / rate;
        let wave: f32 = freqs
            .iter()
            .map(|f| {
                let w = std::f32::consts::TAU * f * t;
                w.sin() + 0.3 * (3.0 * w).sin()
            })
            .sum::<f32>()
            / (freqs.len() as f32 * 1.3);
        let env = (i.min(n - i) as f32 / fade as f32).min(1.0);
        out.push((wave * env * amp * i16::MAX as f32) as i16);
    }
}

fn silence(out: &mut Vec<i16>, ms: u32) {
    out.extend(std::iter::repeat_n(0, SAMPLE_RATE as usize * ms as usize / 1000));
}

/// ponytail: a long file is cut here; a ring or chirp is seconds long anyway.
const MAX_SECONDS: usize = 30;

/// A sound file as Mesh's playback wants it: 48 kHz mono.
pub fn decode(path: &str) -> Result<Vec<i16>, String> {
    use symphonia::core::{audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("not a sound Mesh can read: {e}"))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no audio in it")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let (mut mono, mut rate) = (Vec::new(), SAMPLE_RATE);
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else { continue };
        let spec = *decoded.spec();
        rate = spec.rate;
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        mono.extend(buf.samples().chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
        if mono.len() > rate as usize * MAX_SECONDS {
            break;
        }
    }
    if mono.is_empty() {
        return Err("no audio in it".into());
    }
    Ok(resample(&mono, rate))
}

/// Linear resampling to `SAMPLE_RATE`; fine for notification sounds.
fn resample(mono: &[f32], rate: u32) -> Vec<i16> {
    let step = rate as f64 / SAMPLE_RATE as f64;
    let len = (mono.len() as f64 / step) as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * step;
            let (j, frac) = (pos as usize, (pos.fract()) as f32);
            let a = mono[j.min(mono.len() - 1)];
            let b = mono[(j + 1).min(mono.len() - 1)];
            ((a + (b - a) * frac).clamp(-1.0, 1.0) * i16::MAX as f32) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sounds_are_distinct_and_keyed() {
        let keys: std::collections::HashSet<String> = Sound::ALL.iter().map(|s| s.key()).collect();
        assert_eq!(keys.len(), Sound::ALL.len());
        for s in Sound::ALL {
            assert!(!builtin(s).is_empty(), "{s:?}");
        }
        // Rings differ per network, so you can tell who's calling from where.
        assert_ne!(builtin(Sound::Ring(Network::Discord)), builtin(Sound::Ring(Network::WhatsApp)));
        assert_ne!(builtin(Sound::Message(Network::Discord)), builtin(Sound::Message(Network::Matrix)));
    }

    #[test]
    fn decodes_a_wav_to_48k_mono() {
        // One second of 8 kHz stereo 16-bit PCM.
        let (rate, frames) = (8000u32, 8000usize);
        let data: Vec<u8> = (0..frames).flat_map(|i| {
            let s = ((i as f32 / 10.0).sin() * 10000.0) as i16;
            [s.to_le_bytes(), s.to_le_bytes()].concat()
        }).collect();
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + data.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(2u16.to_le_bytes()); // stereo
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        let path = std::env::temp_dir().join(format!("mesh-sound-test-{}.wav", std::process::id()));
        std::fs::write(&path, wav).unwrap();
        let pcm = decode(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!((pcm.len() as i64 - SAMPLE_RATE as i64).abs() < 100, "{}", pcm.len());
        assert!(pcm.iter().any(|&s| s.abs() > 5000));
    }
}
