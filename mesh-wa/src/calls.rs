//! One call at a time (1:1 or group), reported to Mesh as `waCall`. Their video arrives as
//! `waVideo` JPEGs; ours (the camera, or a shared screen or window) is captured and encoded by
//! mesh-media, and Mesh sees it as `waVideo` with `local: true`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use log::{info, warn};
use mesh_media::{Capture, H264Decoder, H264Encoder, Opts};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use whatsapp_rust::prelude::*;
use whatsapp_rust::voip::{CallEvent, CallHandle, TimedVideoFrame, VideoFrame, VideoSource, VideoUpgradeToken};
use whatsapp_rust::wacore::types::call::{IncomingCall, VideoState};
use whatsapp_rust::wacore::voip_control::signaling::CallPhase;
use whatsapp_rust::wacore_binary::JidExt as _;

use crate::audio::{spawn_mic, spawn_speaker};
use crate::emit;

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
    /// The peer's side of the video changed (an ask to upgrade carries its token).
    PeerVideo(u64, VideoState, Option<VideoUpgradeToken>),
    /// Our video couldn't start (or theirs couldn't be taken).
    VideoFailed(u64, String),
}

pub struct Call {
    pub seq: u64,
    pub info: Info,
    pub incoming: Option<Box<IncomingCall>>,
    pub handle: Option<Arc<CallHandle>>,
    pub video: Video,
}

pub fn call(seq: u64, info: Info, incoming: Option<Box<IncomingCall>>) -> Call {
    Call { seq, info, incoming, handle: None, video: Video::new() }
}

pub fn info(id: &str, peer: &str, name: String, phase: &str) -> Info {
    Info { id: id.into(), peer: peer.into(), name, phase: phase.into(), ..Default::default() }
}

/// What we send as our video.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Camera,
    /// A mesh-media source id (`""`: the main screen).
    Screen(String),
}

const FPS: u32 = 15;
const MAX_WIDTH: u32 = 1280;
const CAMERA_BITRATE: u32 = 1_200_000;
/// Text needs more bits than faces.
const SCREEN_BITRATE: u32 = 1_800_000;
/// The id Mesh knows our own picture by, and a 1:1 peer's.
const SELF_ID: &str = "wa-self";
pub const PEER_ID: &str = "wa-peer";

/// A call's video, both ways.
pub struct Video {
    /// Their pictures, to a decoding thread for as long as the call lasts.
    pub sink: async_channel::Sender<VideoFrame>,
    /// The engine wants a keyframe from us.
    pub keyframe: Arc<AtomicBool>,
    /// Our direction, while the call has it: what the sending thread feeds.
    feed: Option<async_channel::Sender<TimedVideoFrame>>,
    /// What we're sending, and the thread doing it (it stops when dropped).
    sending: Option<(Source, Sending)>,
    /// Where our video's RTP clock starts, so it keeps counting up across a change of source.
    epoch: Instant,
}

/// Our video as the call takes it: encoded access units with their capture time.
pub struct Feed(async_channel::Receiver<TimedVideoFrame>);

impl VideoSource for Feed {
    fn frames(&self) -> async_channel::Receiver<Vec<u8>> {
        async_channel::bounded(1).1
    }

    fn timed_frames(&self) -> Option<async_channel::Receiver<TimedVideoFrame>> {
        Some(self.0.clone())
    }
}

impl Video {
    fn new() -> Video {
        Video { sink: video_sink(), keyframe: Arc::default(), feed: None, sending: None, epoch: Instant::now() }
    }

    /// Opens our direction: the feed to hand the call, kept fed by whatever we send.
    pub fn attach(&mut self) -> Feed {
        let (tx, rx) = async_channel::bounded(4);
        self.feed = Some(tx);
        Feed(rx)
    }

    fn send(&mut self, source: Source) -> anyhow::Result<()> {
        self.sending = None;
        let feed = self.feed.clone().ok_or_else(|| anyhow::anyhow!("no video feed"))?;
        // ponytail: opening a camera can take a second; that holds up the main loop (not the
        // call, which runs on its own tasks). Open it off the loop if that ever shows.
        let sending = tokio::task::block_in_place(|| Sending::start(&source, feed, self.keyframe.clone(), self.epoch))?;
        self.sending = Some((source, sending));
        Ok(())
    }

    /// Stops sending and lets go of our direction.
    pub fn stop(&mut self) {
        self.sending = None;
        self.feed = None;
    }
}

impl Call {
    /// Sends `want` as our video (or nothing): switching what's sent, opening our direction in
    /// the call (re-adding it, or asking to upgrade an audio call), or closing it.
    pub fn set_video(&mut self, want: Option<Source>, tx: &UnboundedSender<Update>) -> Result<(), String> {
        if self.video.sending.as_ref().map(|(s, _)| s) == want.as_ref() {
            return Ok(());
        }
        let was_screen = matches!(self.video.sending, Some((Source::Screen(_), _)));
        let is_screen = matches!(want, Some(Source::Screen(_)));
        self.video.sending = None;
        if self.info.group
            && was_screen != is_screen
            && let Some(handle) = self.handle.clone()
        {
            // Group calls also say who's sharing; the video carries the picture either way.
            tokio::spawn(async move {
                let said = if is_screen { handle.start_screen_share(None).await } else { handle.stop_screen_share().await };
                if let Err(e) = said {
                    warn!("couldn't announce the screen share: {e}");
                }
            });
        }
        let Some(source) = want else {
            // A call that isn't up yet keeps the feed it started with; it just goes quiet.
            if let Some(handle) = self.handle.clone()
                && self.video.feed.take().is_some()
            {
                tokio::spawn(async move {
                    if let Err(e) = handle.stop_video().await {
                        warn!("stop_video: {e}");
                    }
                });
            }
            return Ok(());
        };
        if self.video.feed.is_some() {
            return self.video.send(source).map_err(|e| format!("{e:#}"));
        }
        let Some(handle) = self.handle.clone() else { return Err("wait until the call connects".into()) };
        let feed = self.video.attach();
        if let Err(e) = self.video.send(source) {
            self.video.stop();
            return Err(format!("{e:#}"));
        }
        let (sink, seq, tx) = (self.video.sink.clone(), self.seq, tx.clone());
        tokio::spawn(async move {
            let (me, peer) = handle.video_states().unwrap_or((VideoState::Disabled, VideoState::Disabled));
            // Theirs is on (or ours was): re-add ours. Neither: ask to make it a video call.
            let started = if me == VideoState::Stopped || (me.is_inactive_for_call_mode() && !peer.is_inactive_for_call_mode()) {
                handle.resume_video(feed, sink).await
            } else {
                handle.start_video(feed, sink).await
            };
            if let Err(e) = started {
                let _ = tx.send(Update::VideoFailed(seq, e.to_string()));
            }
        });
        Ok(())
    }

    /// The peer turned video on in a call that had none: take it, so their picture shows, but
    /// keep ours off until our camera or screen is turned on.
    pub fn accept_peer_video(&mut self, token: VideoUpgradeToken, tx: &UnboundedSender<Update>) {
        let Some(handle) = self.handle.clone() else { return };
        if self.video.feed.is_some() {
            return; // we asked at the same moment; the library has settled it
        }
        self.info.video = true;
        let (feed, sink, seq, tx) = (Feed(async_channel::bounded(1).1), self.video.sink.clone(), self.seq, tx.clone());
        tokio::spawn(async move {
            let accepted = async {
                handle.accept_video(token, feed, sink).await?;
                handle.stop_video().await
            };
            if let Err(e) = accepted.await {
                let _ = tx.send(Update::VideoFailed(seq, format!("couldn't take their video: {e}")));
            }
        });
    }
}

/// Capturing, encoding and feeding our video (and showing it to Mesh), on a thread of its own.
struct Sending {
    stop: Arc<AtomicBool>,
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Sending {
    /// Starts capture now, so a camera that can't open says so to the caller.
    fn start(source: &Source, feed: async_channel::Sender<TimedVideoFrame>, keyframe: Arc<AtomicBool>, epoch: Instant) -> anyhow::Result<Sending> {
        let opts = Opts { fps: FPS, max_width: MAX_WIDTH };
        let (capture, bitrate) = match source {
            Source::Camera => (Capture::camera("", opts)?, CAMERA_BITRATE),
            Source::Screen(id) => (Capture::source(id, opts)?, SCREEN_BITRATE),
        };
        let mut encoder = H264Encoder::new(FPS, bitrate)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new().name("mesh-wa video out".into()).spawn(move || {
            let self_gap = Duration::from_millis(1000 / 10);
            let mut shown = Instant::now() - self_gap;
            while let Some(frame) = capture.recv() {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                // A new encoder's first frame is a keyframe anyway.
                let data = match encoder.encode(&frame, keyframe.swap(false, Ordering::Relaxed)) {
                    Ok(data) if !data.is_empty() => data,
                    Ok(_) => continue, // rate control skipped it
                    Err(e) => {
                        log::debug!("video frame didn't encode: {e}");
                        continue;
                    }
                };
                // The 90 kHz RTP clock, from when the frame was taken.
                let timestamp = (frame.at.duration_since(epoch).as_micros() as u64 * 9 / 100) as u32;
                match feed.try_send(TimedVideoFrame::builder().data(data).timestamp(timestamp).build()) {
                    Ok(()) => {}
                    // A dropped frame breaks the chain the next ones build on.
                    Err(async_channel::TrySendError::Full(_)) => keyframe.store(true, Ordering::Relaxed),
                    Err(async_channel::TrySendError::Closed(_)) => break,
                }
                if shown.elapsed() >= self_gap {
                    shown = Instant::now();
                    if let Ok(jpeg) = frame.jpeg(640, 70) {
                        use base64::Engine as _;
                        emit("waVideo", &serde_json::json!({ "id": SELF_ID, "local": true, "jpeg": base64::engine::general_purpose::STANDARD.encode(jpeg) }));
                    }
                }
            }
            emit("waVideo", &serde_json::json!({ "id": SELF_ID, "ended": true }));
        })?;
        Ok(Sending { stop })
    }
}

/// A video call's ends for the library: our feed and their sink.
pub type Ends = Option<(Feed, async_channel::Sender<VideoFrame>)>;

/// Places a call (a group's when `peer` is one); reports `Started` then phases.
pub fn dial(client: Arc<Client>, peer: Jid, video: Ends, keyframe: Arc<AtomicBool>, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            let voip = client.voip();
            let started = if peer.is_group() {
                let call = voip.group_call_by_id(&peer).audio(mic, speaker);
                match video {
                    Some((feed, sink)) => call.video(feed, sink).start().await,
                    None => call.start().await,
                }
            } else {
                let call = voip.call(&peer).audio(mic, speaker);
                match video {
                    Some((feed, sink)) => call.video(feed, sink).start().await,
                    None => call.start().await,
                }
            };
            started.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        report(started, seq, true, keyframe, tx);
    });
}

/// Answers a ringing call with the mic and speaker (and video, given its ends).
pub fn answer(client: Arc<Client>, call: Box<IncomingCall>, video: Ends, keyframe: Arc<AtomicBool>, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            let voip = client.voip();
            let accept = voip.accept(&call).audio(mic, speaker);
            let started = match video {
                Some((feed, sink)) => accept.video(feed, sink).start().await,
                None => accept.start().await,
            };
            started.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        if started.is_err() {
            let _ = client.voip().reject(&call).await;
        }
        report(started, seq, false, keyframe, tx);
    });
}

fn report(started: anyhow::Result<CallHandle>, seq: u64, outgoing: bool, keyframe: Arc<AtomicBool>, tx: UnboundedSender<Update>) {
    match started {
        Ok(handle) => {
            let handle = Arc::new(handle);
            let _ = tx.send(Update::Started(seq, Ok(handle.clone())));
            tokio::spawn(watch(handle, seq, outgoing, keyframe, tx));
        }
        Err(e) => {
            let _ = tx.send(Update::Started(seq, Err(e.to_string())));
        }
    }
}

/// Follows the call's phase until it ends, and its video events.
// ponytail: polls phase every 500 ms; CallEvents doesn't carry every phase change.
async fn watch(handle: Arc<CallHandle>, seq: u64, outgoing: bool, keyframe: Arc<AtomicBool>, tx: UnboundedSender<Update>) {
    if let Some(mut events) = handle.take_events() {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                match event {
                    CallEvent::RelayAllocateFailed(code) => warn!("call relay rejected the allocate (STUN {code})"),
                    CallEvent::RelayAllocateTimedOut => warn!("call relay never answered the allocate"),
                    CallEvent::VideoKeyframeNeeded => keyframe.store(true, Ordering::Relaxed),
                    CallEvent::PeerVideoStateChanged { state, upgrade_token, .. } => {
                        let _ = tx.send(Update::PeerVideo(seq, state, upgrade_token));
                    }
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
        let mut decoders: HashMap<String, (H264Decoder, Instant)> = HashMap::new();
        let gap = Duration::from_millis(1000 / 15);
        while let Ok(frame) = rx.recv_blocking() {
            let id = frame.pid.map_or_else(|| PEER_ID.into(), |p| format!("wa-{p}"));
            if !decoders.contains_key(&id) {
                match H264Decoder::new() {
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
            // Every frame is decoded (the next ones build on it); only those shown become pictures.
            let decoded = if sent.elapsed() >= gap { decoder.decode(&frame.data) } else { decoder.feed(&frame.data).map(|()| None) };
            let picture = match decoded {
                Ok(Some(picture)) => picture,
                Ok(None) => continue,
                Err(e) => {
                    log::debug!("video frame didn't decode: {e}");
                    continue;
                }
            };
            *sent = Instant::now();
            match picture.rotated(frame.orientation).jpeg(640, 70) {
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn nal_types(au: &[u8]) -> Vec<u8> {
        au.windows(5).filter(|w| w[..4] == [0, 0, 0, 1]).map(|w| w[4] & 0x1f).collect()
    }

    #[test]
    fn sends_the_screen_as_timed_h264() {
        let (tx, rx) = async_channel::bounded(4);
        let keyframe = Arc::new(AtomicBool::new(false));
        let sending = Sending::start(&Source::Screen(String::new()), tx, keyframe.clone(), Instant::now()).unwrap();
        let first = rx.recv_blocking().unwrap();
        assert!(nal_types(&first.data).starts_with(&[7, 8, 5]), "{:?}", nal_types(&first.data));
        let second = rx.recv_blocking().unwrap();
        assert!(second.timestamp > first.timestamp && !nal_types(&second.data).contains(&5));
        // Asked for a keyframe, the next frame (or the one after, if one was already queued) is one.
        keyframe.store(true, Ordering::Relaxed);
        assert!((0..3).any(|_| nal_types(&rx.recv_blocking().unwrap().data).contains(&5)));
        let picture = H264Decoder::new().unwrap().decode(&first.data).unwrap().expect("a picture");
        assert!(picture.width <= MAX_WIDTH);
        drop(sending);
        // It stops: the channel closes once the thread lets go of it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while rx.recv_blocking().is_ok() {
            assert!(Instant::now() < deadline, "still sending");
        }
    }
}
