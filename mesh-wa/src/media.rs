//! What Mesh shows of WhatsApp's media: shrunk pictures, GIF frames, voice notes as PCM.

use std::io::Cursor;

use anyhow::{Context as _, Result, anyhow, bail};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbImage};
use mesh_media::H264Decoder;

/// Shrinks a picture so Mesh gets a few KB: photos to `width` px as JPEG, stickers to 160 px as
/// PNG (keeping their transparency). An animated sticker goes as it is, for Mesh to play.
pub fn shrink(bytes: &[u8], sticker: bool, width: u32) -> Result<Vec<u8>> {
    if sticker && image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).is_ok_and(|d| d.has_animation()) {
        return Ok(bytes.to_vec());
    }
    let img = image::load_from_memory(bytes).context("can't read the picture")?;
    let max = if sticker { 160 } else { width };
    let img = fit(img, max);
    if sticker {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png)?;
        Ok(out.into_inner())
    } else {
        jpeg(&img.to_rgb8(), 80)
    }
}

pub fn jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(img)?;
    Ok(out)
}

/// A photo we send: its preview JPEG (what the chat shows until it's downloaded) and its size.
pub fn preview(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    Some((jpeg(&fit(img, 96).to_rgb8(), 60).ok()?, w, h))
}

pub const GIF_FPS: u32 = 12;
const GIF_SECONDS: u32 = 6;
const GIF_WIDTH: u32 = 360;

/// A GIF's frames (it's an H.264 mp4): up to 6 s at 12 fps, as 360 px wide JPEGs.
// ponytail: assumes 4-byte NAL lengths (what WhatsApp's encoders write); OpenH264 decodes
// baseline/main, so a high-profile GIF fails here and Mesh just shows nothing.
pub fn gif_frames(mp4: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut reader = mp4::Mp4Reader::read_header(Cursor::new(mp4), mp4.len() as u64).map_err(|e| anyhow!("not an mp4: {e}"))?;
    let track = reader
        .tracks()
        .values()
        .find(|t| t.media_type().is_ok_and(|m| m == mp4::MediaType::H264))
        .ok_or_else(|| anyhow!("no H.264 video in it"))?;
    let (id, timescale, count) = (track.track_id(), track.timescale().max(1) as u64, track.sample_count());
    let mut stream = Vec::new();
    for nal in [track.sequence_parameter_set(), track.picture_parameter_set()] {
        stream.extend_from_slice(&[0, 0, 0, 1]);
        stream.extend_from_slice(nal.map_err(|e| anyhow!("{e}"))?);
    }
    let mut decoder = H264Decoder::new()?;
    let (mut frames, mut next_ms) = (Vec::new(), 0u64);
    for n in 1..=count {
        let Some(sample) = reader.read_sample(id, n).map_err(|e| anyhow!("{e}"))? else { continue };
        let at_ms = sample.start_time * 1000 / timescale;
        if at_ms >= u64::from(GIF_SECONDS) * 1000 {
            break;
        }
        annex_b(&sample.bytes, &mut stream);
        let Some(frame) = decoder.decode(&stream)? else {
            stream.clear();
            continue;
        };
        stream.clear();
        if at_ms < next_ms {
            continue;
        }
        next_ms = at_ms + 1000 / u64::from(GIF_FPS);
        frames.push(frame.jpeg(GIF_WIDTH, 75)?);
    }
    if frames.is_empty() {
        bail!("couldn't decode the GIF");
    }
    Ok(frames)
}

fn fit(img: DynamicImage, width: u32) -> DynamicImage {
    if img.width() > width { img.resize(width, u32::MAX, FilterType::Triangle) } else { img }
}

/// Length-prefixed (AVCC) NAL units to start-code (Annex B) ones, appended to `out`.
fn annex_b(mut avcc: &[u8], out: &mut Vec<u8>) {
    while avcc.len() >= 4 {
        let len = u32::from_be_bytes([avcc[0], avcc[1], avcc[2], avcc[3]]) as usize;
        let Some(nal) = avcc.get(4..4 + len) else { break };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        avcc = &avcc[4 + len..];
    }
}

/// The packets of an Ogg stream (one logical stream: what voice notes are).
fn ogg_packets(mut ogg: &[u8]) -> Result<Vec<Vec<u8>>> {
    let (mut packets, mut packet) = (Vec::new(), Vec::new());
    while !ogg.is_empty() {
        if ogg.len() < 27 || &ogg[..4] != b"OggS" {
            bail!("not an Ogg file");
        }
        let segments = ogg[26] as usize;
        let table = ogg.get(27..27 + segments).ok_or_else(|| anyhow!("cut-off Ogg page"))?;
        let mut data = &ogg[27 + segments..];
        for &lace in table {
            let lace = lace as usize;
            let chunk = data.get(..lace).ok_or_else(|| anyhow!("cut-off Ogg page"))?;
            packet.extend_from_slice(chunk);
            data = &data[lace..];
            if lace < 255 {
                packets.push(std::mem::take(&mut packet));
            }
        }
        ogg = data;
    }
    Ok(packets)
}

/// A voice note (Ogg Opus) as 48 kHz mono samples, what Mesh's speaker output plays.
pub fn voice_pcm(ogg: &[u8]) -> Result<Vec<i16>> {
    let packets = ogg_packets(ogg)?;
    let head = packets.first().filter(|p| p.starts_with(b"OpusHead") && p.len() >= 19).ok_or_else(|| anyhow!("not an Opus voice note"))?;
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
    // Opus decodes any stream to mono itself.
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono)?;
    let (mut pcm, mut buf) = (Vec::new(), vec![0i16; 5760]);
    for packet in packets.iter().skip(2).filter(|p| !p.is_empty()) {
        let n = decoder.decode(packet, &mut buf, false)?;
        pcm.extend_from_slice(&buf[..n]);
    }
    Ok(pcm.split_off(pre_skip.min(pcm.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One Ogg page holding `packets`.
    fn page(packets: &[&[u8]]) -> Vec<u8> {
        let mut table = Vec::new();
        for p in packets {
            table.extend(std::iter::repeat_n(255u8, p.len() / 255));
            table.push((p.len() % 255) as u8);
        }
        let mut page = b"OggS".to_vec();
        page.extend_from_slice(&[0; 22]);
        page.push(table.len() as u8);
        page.extend(table);
        packets.iter().for_each(|p| page.extend_from_slice(p));
        page
    }

    #[test]
    fn decodes_an_ogg_opus_voice_note() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 1, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]); // 1 channel, pre-skip 312
        let mut enc = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let tone: Vec<i16> = (0..960).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
        let frames: Vec<Vec<u8>> = (0..5).map(|_| enc.encode_vec(&tone, 4000).unwrap()).collect();
        let long = vec![7u8; 300]; // a packet over one 255-byte lace
        let mut ogg = page(&[&head, b"OpusTags"]);
        ogg.extend(page(&frames.iter().map(Vec::as_slice).collect::<Vec<_>>()));
        assert_eq!(ogg_packets(&page(&[&long, b"x"])).unwrap(), vec![long.clone(), b"x".to_vec()]);
        let pcm = voice_pcm(&ogg).unwrap();
        assert_eq!(pcm.len(), 5 * 960 - 312);
        assert!(voice_pcm(b"RIFF....").is_err());
    }

    /// An H.264 mp4 of `n` frames at 12 fps, as WhatsApp's GIFs are.
    fn gif_mp4(n: u32) -> Vec<u8> {
        let (w, h) = (64u16, 48u16);
        let mut enc = mesh_media::H264Encoder::new(12, 1_000_000).unwrap();
        let (mut sps, mut pps, mut samples) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            let rgba = image::RgbaImage::from_fn(w.into(), h.into(), |x, _| image::Rgba([(x * 4) as u8, (i * 20) as u8, 0, 255]));
            let frame = mesh_media::Frame { width: w.into(), height: h.into(), rgba: rgba.into_raw(), at: std::time::Instant::now() };
            let annexb = enc.encode(&frame, false).unwrap();
            let mut sample = Vec::new();
            for nal in split_nals(&annexb) {
                match nal[0] & 0x1f {
                    7 => sps = nal.to_vec(),
                    8 => pps = nal.to_vec(),
                    _ => {
                        sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        sample.extend_from_slice(nal);
                    }
                }
            }
            samples.push(sample);
        }
        let config = mp4::Mp4Config { major_brand: "isom".parse().unwrap(), minor_version: 512, compatible_brands: vec![], timescale: 1000 };
        let mut writer = mp4::Mp4Writer::write_start(Cursor::new(Vec::new()), &config).unwrap();
        let avc = mp4::AvcConfig { width: w, height: h, seq_param_set: sps, pic_param_set: pps };
        let track = mp4::TrackConfig { track_type: mp4::TrackType::Video, timescale: 1000, language: "und".into(), media_conf: mp4::MediaConfig::AvcConfig(avc) };
        writer.add_track(&track).unwrap();
        for (i, bytes) in samples.into_iter().enumerate() {
            let sample = mp4::Mp4Sample { start_time: i as u64 * 83, duration: 83, rendering_offset: 0, is_sync: i == 0, bytes: bytes.into() };
            writer.write_sample(1, &sample).unwrap();
        }
        writer.write_end().unwrap();
        writer.into_writer().into_inner()
    }

    fn split_nals(annexb: &[u8]) -> Vec<&[u8]> {
        let starts: Vec<usize> = (0..annexb.len().saturating_sub(3)).filter(|&i| annexb[i..i + 3] == [0, 0, 1]).collect();
        starts
            .iter()
            .enumerate()
            .map(|(k, &s)| {
                let end = starts.get(k + 1).map_or(annexb.len(), |&e| if annexb[e - 1] == 0 { e - 1 } else { e });
                &annexb[s + 3..end]
            })
            .collect()
    }

    #[test]
    fn decodes_gif_frames() {
        let frames = gif_frames(&gif_mp4(24)).unwrap();
        assert_eq!(frames.len(), 24);
        assert_eq!(image::guess_format(&frames[0]).unwrap(), ImageFormat::Jpeg);
        assert!(gif_frames(b"not an mp4").is_err());
    }

    #[test]
    fn shrinks_photos_and_stickers() {
        let big = DynamicImage::ImageRgb8(RgbImage::new(1000, 500));
        let mut png = Cursor::new(Vec::new());
        big.write_to(&mut png, ImageFormat::Png).unwrap();
        let photo = shrink(png.get_ref(), false, 480).unwrap();
        assert_eq!(image::guess_format(&photo).unwrap(), ImageFormat::Jpeg);
        let size = |b: &[u8]| image::load_from_memory(b).map(|i| (i.width(), i.height())).ok();
        assert_eq!(size(&photo), Some((480, 240)));
        let sticker = shrink(png.get_ref(), true, 480).unwrap();
        assert_eq!((image::guess_format(&sticker).unwrap(), size(&sticker)), (ImageFormat::Png, Some((160, 80))));
        let (thumb, w, h) = preview(png.get_ref()).unwrap();
        assert_eq!((size(&thumb), w, h), (Some((96, 48)), 1000, 500));

        let mut out = Vec::new();
        annex_b(&[0, 0, 0, 2, 9, 9, 0, 0, 0, 1, 5], &mut out);
        assert_eq!(out, [0, 0, 0, 1, 9, 9, 0, 0, 0, 1, 5]);
    }
}
