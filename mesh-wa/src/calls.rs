//! One call at a time (1:1 or group), reported to Mesh as `waCall`. Video calls show the other
//! side's camera (`waVideo`); we send none, having no camera capture.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use whatsapp_rust::prelude::*;
use whatsapp_rust::wacore_binary::JidExt as _;
use whatsapp_rust::voip::{CallEvent, CallHandle, VideoFrame};
use whatsapp_rust::wacore::types::call::IncomingCall;
use whatsapp_rust::wacore::voip_control::signaling::CallPhase;

use crate::audio::{spawn_mic, spawn_speaker};
use crate::{emit, media};

/// Mesh's `Call`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Info {
    pub id: String,
    pub peer: String,
    pub name: String,
    pub phase: String,
    pub video: bool,
    pub group: bool,
    pub muted: bool,
    pub camera: bool,
    pub screen: bool,
}

/// What call tasks report back to the main loop. `seq` tells a stale task from the current call.
pub enum Update {
    Started(u64, Result<Arc<CallHandle>, String>),
    Phase(u64, &'static str),
    Ended(u64),
}

pub struct Call {
    pub seq: u64,
    pub info: Info,
    pub incoming: Option<Box<IncomingCall>>,
    pub handle: Option<Arc<CallHandle>>,
}

pub fn info(id: &str, peer: &str, name: String, phase: &str) -> Info {
    Info { id: id.into(), peer: peer.into(), name, phase: phase.into(), ..Default::default() }
}

/// Our camera in a video call: one that never sends, kept open for as long as the call is.
struct Video {
    _camera: async_channel::Sender<Vec<u8>>,
}

fn video_ends() -> (Video, async_channel::Receiver<Vec<u8>>, async_channel::Sender<VideoFrame>) {
    let (camera, frames) = async_channel::bounded(1);
    (Video { _camera: camera }, frames, video_sink())
}

/// Places a call (a group's when `peer` is one); reports `Started` then phases.
pub fn dial(client: Arc<Client>, peer: Jid, video: bool, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let mut ours = None;
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            let voip = client.voip();
            let started = if peer.is_group() {
                let call = voip.group_call_by_id(&peer).audio(mic, speaker);
                match video.then(video_ends) {
                    Some((v, cam, sink)) => {
                        ours = Some(v);
                        call.video(cam, sink).start().await
                    }
                    None => call.start().await,
                }
            } else {
                let call = voip.call(&peer).audio(mic, speaker);
                match video.then(video_ends) {
                    Some((v, cam, sink)) => {
                        ours = Some(v);
                        call.video(cam, sink).start().await
                    }
                    None => call.start().await,
                }
            };
            started.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        report(started, seq, true, ours, tx);
    });
}

/// Answers a ringing call with the mic and speaker (and their camera, if `video`).
pub fn answer(client: Arc<Client>, call: Box<IncomingCall>, video: bool, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let mut ours = None;
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            let voip = client.voip();
            let accept = voip.accept(&call).audio(mic, speaker);
            let started = match video.then(video_ends) {
                Some((v, cam, sink)) => {
                    ours = Some(v);
                    accept.video(cam, sink).start().await
                }
                None => accept.start().await,
            };
            started.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        if started.is_err() {
            let _ = client.voip().reject(&call).await;
        }
        report(started, seq, false, ours, tx);
    });
}

fn report(started: anyhow::Result<CallHandle>, seq: u64, outgoing: bool, video: Option<Video>, tx: UnboundedSender<Update>) {
    match started {
        Ok(handle) => {
            let handle = Arc::new(handle);
            let _ = tx.send(Update::Started(seq, Ok(handle.clone())));
            tokio::spawn(async move {
                watch(handle, seq, outgoing, tx).await;
                drop(video);
            });
        }
        Err(e) => {
            let _ = tx.send(Update::Started(seq, Err(e.to_string())));
        }
    }
}

/// Follows the call's phase until it ends.
// ponytail: polls phase every 500 ms; CallEvents doesn't carry every phase change.
async fn watch(handle: Arc<CallHandle>, seq: u64, outgoing: bool, tx: UnboundedSender<Update>) {
    if let Some(mut events) = handle.take_events() {
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                match event {
                    CallEvent::RelayAllocateFailed(code) => warn!("call relay rejected the allocate (STUN {code})"),
                    CallEvent::RelayAllocateTimedOut => warn!("call relay never answered the allocate"),
                    _ => {}
                }
            }
        });
    }
    let ended = handle.wait_ended();
    tokio::pin!(ended);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let (mut last, mut active_since, mut warned) = ("", None::<Instant>, false);
    loop {
        tokio::select! {
            _ = &mut ended => break,
            _ = tick.tick() => {}
        }
        let phase = match handle.phase() {
            None | Some(CallPhase::Ended) => break,
            Some(CallPhase::Active) if handle.media_connected() => "active",
            Some(CallPhase::Calling | CallPhase::Ringing) if outgoing => "outgoing",
            _ => "connecting",
        };
        if phase != last {
            last = phase;
            let _ = tx.send(Update::Phase(seq, phase));
        }
        // Library issue #1098: an outgoing call can connect with no inbound audio.
        if outgoing && phase == "active" && !warned {
            let since = *active_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(8) && handle.media_stats().audio_frames_decoded == 0 {
                warn!("call {} is active but no inbound audio arrived in 8 s (whatsapp-rust #1098)", handle.call_id());
                warned = true;
            }
        }
    }
    info!("call {} ended", handle.call_id());
    let _ = tx.send(Update::Ended(seq));
}

/// Their camera(s): decodes the call's H.264 on a thread and sends Mesh `waVideo` JPEGs, up to
/// 15 a second each, until the call drops its end.
// ponytail: a decode error waits for the next keyframe instead of asking for one (the handle
// isn't there yet when this starts); `orientation` is taken as clockwise quarter turns.
fn video_sink() -> async_channel::Sender<VideoFrame> {
    let (tx, rx) = async_channel::bounded::<VideoFrame>(30);
    std::thread::spawn(move || {
        let mut decoders: HashMap<String, (openh264::decoder::Decoder, Instant)> = HashMap::new();
        let gap = Duration::from_millis(1000 / 15);
        while let Ok(frame) = rx.recv_blocking() {
            let id = format!("wa-{}", frame.pid.map_or_else(|| "peer".into(), |p| p.to_string()));
            if !decoders.contains_key(&id) {
                match openh264::decoder::Decoder::new() {
                    Ok(d) => {
                        decoders.insert(id.clone(), (d, Instant::now() - gap));
                    }
                    Err(e) => {
                        warn!("no H.264 decoder: {e}");
                        continue;
                    }
                }
            }
            let Some((decoder, sent)) = decoders.get_mut(&id) else { continue };
            let yuv = match decoder.decode(&frame.data) {
                Ok(Some(yuv)) if sent.elapsed() >= gap => yuv,
                Ok(_) => continue,
                Err(e) => {
                    log::debug!("video frame didn't decode: {e}");
                    continue;
                }
            };
            *sent = Instant::now();
            match media::yuv_jpeg(&yuv, frame.orientation) {
                Ok(jpeg) => {
                    use base64::Engine as _;
                    emit("waVideo", &serde_json::json!({ "id": id, "local": false, "jpeg": base64::engine::general_purpose::STANDARD.encode(jpeg) }));
                }
                Err(e) => log::debug!("video frame: {e}"),
            }
        }
        for id in decoders.keys() {
            emit("waVideo", &serde_json::json!({ "id": id, "ended": true }));
        }
    });
    tx
}
