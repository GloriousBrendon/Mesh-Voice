//! 1:1 voice calls over Matrix VoIP signaling (`m.call.*`) with a pure-Rust
//! WebRTC/Opus media stack.
//!
//! Signaling uses the legacy 1:1 events (`m.call.invite` / `answer` /
//! `candidates` / `hangup`, VoIP version 1). Media is [`webrtc`]: an Opus audio
//! track captured from the mic ([`crate::audio`]) and remote audio decoded to
//! the speaker. ICE uses the homeserver's TURN server (fetched over the client
//! API), so calls traverse NAT.
//!
//! Only one call is active at a time — the single-call model keeps the public
//! surface small: [`MeshClient::place_call`], [`accept_call`], [`hangup`],
//! [`set_call_muted`]. Progress is delivered as [`crate::MeshUpdate`] variants.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use bytes::Bytes;
use matrix_sdk::{
    Client,
    ruma::{
        MilliSecondsSinceUnixEpoch, OwnedRoomId, OwnedUserId, OwnedVoipId, UInt, VoipVersionId,
        api::client::voip::get_turn_server_info,
        events::call::{
            SessionDescription,
            answer::{CallAnswerEventContent, OriginalSyncCallAnswerEvent},
            candidates::{Candidate, CallCandidatesEventContent, OriginalSyncCallCandidatesEvent},
            hangup::{CallHangupEventContent, OriginalSyncCallHangupEvent},
            invite::{CallInviteEventContent, OriginalSyncCallInviteEvent},
            reject::OriginalSyncCallRejectEvent,
        },
    },
};
use tokio::sync::{Mutex, mpsc};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MediaEngine};
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::audio;
use crate::MeshUpdate;

const VOIP_VERSION: VoipVersionId = VoipVersionId::V1;
/// How long an outgoing invite is considered ringing, per the spec (ms).
const CALL_LIFETIME_MS: u32 = 60_000;

/// Owns the current call (if any) and the update channel used to notify the UI.
pub struct CallEngine {
    client: Client,
    active: Mutex<Option<ActiveCall>>,
    updates: StdMutex<Option<mpsc::UnboundedSender<MeshUpdate>>>,
    /// Chosen mic / speaker (by name); `None` = system default.
    input_device: StdMutex<Option<String>>,
    output_device: StdMutex<Option<String>>,
}

struct ActiveCall {
    call_id: String,
    room_id: OwnedRoomId,
    /// Our party id for this call (VoIP v1).
    party_id: String,
    /// Set once we place/answer; `None` while an incoming call is only ringing.
    peer: Option<Arc<RTCPeerConnection>>,
    /// Buffered SDP offer for an incoming call, applied on accept.
    incoming_offer: Option<String>,
    /// Remote ICE candidates that arrived before the remote description was set.
    pending_remote_candidates: Vec<RTCIceCandidateInit>,
    remote_description_set: bool,
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    // Kept alive for the duration of the call; dropping stops the audio devices.
    _capture: Option<audio::CaptureHandle>,
    _playback: Option<audio::PlaybackHandle>,
    _capture_task: Option<tokio::task::JoinHandle<()>>,
}

impl CallEngine {
    pub fn new(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            active: Mutex::new(None),
            updates: StdMutex::new(None),
            input_device: StdMutex::new(None),
            output_device: StdMutex::new(None),
        })
    }

    /// Selects the mic / speaker used for future calls (`None` = default).
    pub fn set_audio_devices(&self, input: Option<String>, output: Option<String>) {
        *self.input_device.lock().unwrap() = input;
        *self.output_device.lock().unwrap() = output;
    }

    pub fn set_update_sender(&self, tx: mpsc::UnboundedSender<MeshUpdate>) {
        *self.updates.lock().unwrap() = Some(tx);
    }

    fn emit(&self, update: MeshUpdate) {
        if let Some(tx) = self.updates.lock().unwrap().as_ref() {
            let _ = tx.send(update);
        }
    }

    /// Registers the sync event handlers for incoming call signaling. Call once.
    pub fn register_handlers(self: &Arc<Self>) {
        let client = self.client.clone();

        let engine = self.clone();
        client.add_event_handler(move |ev: OriginalSyncCallInviteEvent, room: matrix_sdk::Room| {
            let engine = engine.clone();
            async move {
                engine.on_invite(room.room_id().to_owned(), ev).await;
            }
        });

        let engine = self.clone();
        client.add_event_handler(move |ev: OriginalSyncCallAnswerEvent, _room: matrix_sdk::Room| {
            let engine = engine.clone();
            async move {
                engine.on_answer(ev).await;
            }
        });

        let engine = self.clone();
        client.add_event_handler(
            move |ev: OriginalSyncCallCandidatesEvent, _room: matrix_sdk::Room| {
                let engine = engine.clone();
                async move {
                    engine.on_candidates(ev).await;
                }
            },
        );

        let engine = self.clone();
        client.add_event_handler(move |ev: OriginalSyncCallHangupEvent, _room: matrix_sdk::Room| {
            let engine = engine.clone();
            async move {
                engine
                    .on_remote_end(ev.sender, ev.content.call_id.as_str(), "the other party hung up")
                    .await;
            }
        });

        // Element declines v1 calls with m.call.reject rather than a hangup.
        let engine = self.clone();
        client.add_event_handler(move |ev: OriginalSyncCallRejectEvent, _room: matrix_sdk::Room| {
            let engine = engine.clone();
            async move {
                engine
                    .on_remote_end(ev.sender, ev.content.call_id.as_str(), "call declined")
                    .await;
            }
        });
    }

    /// Fetches ICE servers: the homeserver's TURN plus a public STUN fallback.
    async fn ice_servers(&self) -> Vec<RTCIceServer> {
        let mut servers = vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }];
        if let Ok(turn) = self
            .client
            .send(get_turn_server_info::v3::Request::new())
            .await
        {
            if !turn.uris.is_empty() {
                servers.push(RTCIceServer {
                    urls: turn.uris,
                    username: turn.username,
                    credential: turn.password,
                });
            }
        }
        servers
    }

    /// Builds a peer connection with a local Opus audio track and default codecs.
    async fn new_peer(&self) -> Result<(Arc<RTCPeerConnection>, Arc<TrackLocalStaticSample>), String> {
        let mut media = MediaEngine::default();
        media.register_default_codecs().map_err(|e| e.to_string())?;
        let mut registry = Registry::new();
        registry =
            register_default_interceptors(registry, &mut media).map_err(|e| e.to_string())?;
        let api = APIBuilder::new()
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ice_servers: self.ice_servers().await,
            ..Default::default()
        };
        let pc = Arc::new(
            api.new_peer_connection(config)
                .await
                .map_err(|e| e.to_string())?,
        );

        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            "audio".to_owned(),
            "mesh-audio".to_owned(),
        ));
        pc.add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| e.to_string())?;

        Ok((pc, track))
    }

    /// Wires ICE-candidate trickling, remote-track playback, and state changes
    /// onto a freshly built peer connection, and starts mic capture.
    async fn wire_and_start_audio(
        self: &Arc<Self>,
        pc: &Arc<RTCPeerConnection>,
        track: &Arc<TrackLocalStaticSample>,
        call_id: String,
        room_id: OwnedRoomId,
        party_id: String,
        muted: Arc<AtomicBool>,
        deafened: Arc<AtomicBool>,
    ) -> Result<(audio::CaptureHandle, audio::PlaybackHandle, tokio::task::JoinHandle<()>), String>
    {
        // Outgoing ICE candidates -> m.call.candidates.
        let engine = self.clone();
        let cand_call = call_id.clone();
        let cand_room = room_id.clone();
        let cand_party = party_id.clone();
        pc.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            let engine = engine.clone();
            let cand_call = cand_call.clone();
            let cand_room = cand_room.clone();
            let cand_party = cand_party.clone();
            Box::pin(async move {
                let Some(candidate) = candidate else { return };
                let Ok(init) = candidate.to_json() else { return };
                let mut c = Candidate::new(init.candidate);
                c.sdp_mid = init.sdp_mid;
                c.sdp_m_line_index = init.sdp_mline_index.map(UInt::from);
                let mut content = CallCandidatesEventContent::new(
                    OwnedVoipId::from(cand_call),
                    vec![c],
                    VOIP_VERSION,
                );
                content.party_id = Some(OwnedVoipId::from(cand_party));
                if let Some(room) = engine.client.get_room(&cand_room) {
                    let _ = room.send(content).await;
                }
            })
        }));

        // Peer connection lifecycle -> UI updates. `Disconnected` is transient
        // (ICE often recovers from a network blip), so only `Failed` ends the
        // call; `Closed` follows our own hangup, which already notified the UI.
        let engine = self.clone();
        let state_call = call_id.clone();
        pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            let engine = engine.clone();
            let state_call = state_call.clone();
            Box::pin(async move {
                match state {
                    RTCPeerConnectionState::Connected => {
                        engine.emit(MeshUpdate::CallConnected {
                            call_id: state_call,
                        });
                    }
                    RTCPeerConnectionState::Failed => {
                        // Off the callback: end_call closes this peer connection.
                        tokio::spawn(async move {
                            engine.end_call(&state_call, "connection lost").await;
                        });
                    }
                    _ => {}
                }
            })
        }));

        // Playback: remote track -> Opus decode -> shared buffer -> speaker.
        let output_device = self.output_device.lock().unwrap().clone();
        let (playback, buffer) = audio::start_playback(output_device)?;
        pc.on_track(Box::new(move |track, _receiver, _transceiver| {
            let buffer = buffer.clone();
            let deafened = deafened.clone();
            Box::pin(async move {
                spawn_playback_reader(track, buffer, deafened);
            })
        }));

        // Capture: mic -> Opus encode -> local track.
        let input_device = self.input_device.lock().unwrap().clone();
        let (capture, mut frames) = audio::start_capture(input_device)?;
        let track = track.clone();
        let capture_task = tokio::spawn(async move {
            let mut encoder = match opus::Encoder::new(
                audio::SAMPLE_RATE,
                opus::Channels::Mono,
                opus::Application::Voip,
            ) {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("opus encoder: {e}");
                    return;
                }
            };
            let mut out = vec![0u8; 4000];
            while let Some(frame) = frames.recv().await {
                if muted.load(Ordering::Relaxed) {
                    continue;
                }
                match encoder.encode(&frame, &mut out) {
                    Ok(n) => {
                        let sample = Sample {
                            data: Bytes::copy_from_slice(&out[..n]),
                            duration: Duration::from_millis(20),
                            ..Default::default()
                        };
                        if track.write_sample(&sample).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!("opus encode: {e}"),
                }
            }
        });

        Ok((capture, playback, capture_task))
    }

    /// Places an outgoing call to the (assumed 1:1) room.
    pub async fn place_call(self: &Arc<Self>, room_id: &OwnedRoomId) -> Result<(), String> {
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err("a call is already in progress".to_owned());
        }
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| "room not found".to_owned())?;

        // The other member of a 1:1 room.
        let invitee = self.other_member(&room).await?;
        let call_id = new_id();
        let party_id = new_id();
        let muted = Arc::new(AtomicBool::new(false));
        let deafened = Arc::new(AtomicBool::new(false));

        let (pc, track) = self.new_peer().await?;
        let (capture, playback, task) = self
            .wire_and_start_audio(
                &pc,
                &track,
                call_id.clone(),
                room_id.clone(),
                party_id.clone(),
                muted.clone(),
                deafened.clone(),
            )
            .await?;

        let offer = pc.create_offer(None).await.map_err(|e| e.to_string())?;
        pc.set_local_description(offer.clone())
            .await
            .map_err(|e| e.to_string())?;

        let mut content = CallInviteEventContent::version_1(
            OwnedVoipId::from(call_id.clone()),
            OwnedVoipId::from(party_id.clone()),
            CALL_LIFETIME_MS.into(),
            SessionDescription::new("offer".to_owned(), offer.sdp),
        );
        content.invitee = Some(invitee);
        room.send(content).await.map_err(|e| e.to_string())?;

        *active = Some(ActiveCall {
            call_id: call_id.clone(),
            room_id: room_id.clone(),
            party_id,
            peer: Some(pc),
            incoming_offer: None,
            pending_remote_candidates: Vec::new(),
            remote_description_set: false,
            muted,
            deafened,
            _capture: Some(capture),
            _playback: Some(playback),
            _capture_task: Some(task),
        });
        drop(active);
        self.emit(MeshUpdate::CallConnecting {
            call_id: call_id.clone(),
        });

        // Stop ringing once the invite expires unanswered.
        let engine = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(CALL_LIFETIME_MS.into())).await;
            let unanswered = engine
                .active
                .lock()
                .await
                .as_ref()
                .is_some_and(|c| c.call_id == call_id && !c.remote_description_set);
            if unanswered {
                engine.end_call(&call_id, "no answer").await;
            }
        });
        Ok(())
    }

    /// Accepts the currently-ringing incoming call.
    pub async fn accept_call(self: &Arc<Self>) -> Result<(), String> {
        let mut active = self.active.lock().await;
        let Some(call) = active.as_mut() else {
            return Err("no incoming call".to_owned());
        };
        let offer_sdp = call
            .incoming_offer
            .clone()
            .ok_or_else(|| "call has no offer".to_owned())?;
        let call_id = call.call_id.clone();
        let room_id = call.room_id.clone();
        let party_id = call.party_id.clone();
        let muted = call.muted.clone();
        let deafened = call.deafened.clone();

        let (pc, track) = self.new_peer().await?;
        let (capture, playback, task) = self
            .wire_and_start_audio(
                &pc,
                &track,
                call_id.clone(),
                room_id.clone(),
                party_id.clone(),
                muted,
                deafened,
            )
            .await?;

        pc.set_remote_description(
            RTCSessionDescription::offer(offer_sdp).map_err(|e| e.to_string())?,
        )
        .await
        .map_err(|e| e.to_string())?;

        let answer = pc.create_answer(None).await.map_err(|e| e.to_string())?;
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| e.to_string())?;

        // Apply any candidates that arrived while ringing.
        for cand in call.pending_remote_candidates.drain(..) {
            let _ = pc.add_ice_candidate(cand).await;
        }
        call.remote_description_set = true;
        call.peer = Some(pc);
        call.incoming_offer = None;
        call._capture = Some(capture);
        call._playback = Some(playback);
        call._capture_task = Some(task);

        let mut content = CallAnswerEventContent::new(
            SessionDescription::new("answer".to_owned(), answer.sdp),
            OwnedVoipId::from(call_id.clone()),
            VOIP_VERSION,
        );
        content.party_id = Some(OwnedVoipId::from(party_id));
        if let Some(room) = self.client.get_room(&room_id) {
            room.send(content).await.map_err(|e| e.to_string())?;
        }
        self.emit(MeshUpdate::CallConnecting { call_id });
        Ok(())
    }

    /// Hangs up / rejects the current call and sends `m.call.hangup`.
    pub async fn hangup(self: &Arc<Self>) -> Result<(), String> {
        let call_id = self.active.lock().await.as_ref().map(|c| c.call_id.clone());
        if let Some(call_id) = call_id {
            self.end_call(&call_id, "hung up").await;
        }
        Ok(())
    }

    /// Ends `call_id` if it is still the active call: stops media, sends
    /// `m.call.hangup` so the other side stops too, and notifies the UI.
    async fn end_call(&self, call_id: &str, reason: &str) {
        let Some(call) = self.take_if(call_id).await else {
            return;
        };
        if let Some(pc) = &call.peer {
            let _ = pc.close().await;
        }
        let mut content =
            CallHangupEventContent::new(OwnedVoipId::from(call.call_id.clone()), VOIP_VERSION);
        content.party_id = Some(OwnedVoipId::from(call.party_id.clone()));
        if let Some(room) = self.client.get_room(&call.room_id) {
            let _ = room.send(content).await;
        }
        self.emit(MeshUpdate::CallEnded {
            call_id: call.call_id,
            reason: reason.to_owned(),
        });
    }

    pub async fn set_muted(&self, muted: bool) {
        if let Some(call) = self.active.lock().await.as_ref() {
            call.muted.store(muted, Ordering::Relaxed);
        }
    }

    /// Silences remote audio. Mic mute is separate: the UI sets it alongside
    /// (deafen implies mute, like Discord) via [`Self::set_muted`].
    pub async fn set_deafened(&self, deafened: bool) {
        if let Some(call) = self.active.lock().await.as_ref() {
            call.deafened.store(deafened, Ordering::Relaxed);
        }
    }

    async fn on_invite(self: &Arc<Self>, room_id: OwnedRoomId, ev: OriginalSyncCallInviteEvent) {
        // Ignore our own invite echoed back, and invites addressed to someone else.
        if Some(ev.sender.as_ref()) == self.client.user_id() {
            return;
        }
        if let Some(invitee) = &ev.content.invitee {
            if Some(invitee.as_ref()) != self.client.user_id() {
                return;
            }
        }
        // Sync replays invites sent while we were offline; don't ring for those.
        if invite_expired(
            MilliSecondsSinceUnixEpoch::now().get().into(),
            ev.origin_server_ts.get().into(),
            ev.content.lifetime.into(),
        ) {
            return;
        }

        let call_id = ev.content.call_id.to_string();
        let mut active = self.active.lock().await;
        if active.is_some() {
            // Already busy: reject.
            let mut content =
                CallHangupEventContent::new(ev.content.call_id.clone(), VOIP_VERSION);
            content.party_id = Some(OwnedVoipId::from(new_id()));
            if let Some(room) = self.client.get_room(&room_id) {
                let _ = room.send(content).await;
            }
            return;
        }

        *active = Some(ActiveCall {
            call_id: call_id.clone(),
            room_id: room_id.clone(),
            party_id: new_id(),
            peer: None,
            incoming_offer: Some(ev.content.offer.sdp),
            pending_remote_candidates: Vec::new(),
            remote_description_set: false,
            muted: Arc::new(AtomicBool::new(false)),
            deafened: Arc::new(AtomicBool::new(false)),
            _capture: None,
            _playback: None,
            _capture_task: None,
        });
        drop(active);
        self.emit(MeshUpdate::IncomingCall {
            call_id,
            room_id,
            caller: ev.sender,
        });
    }

    async fn on_answer(self: &Arc<Self>, ev: OriginalSyncCallAnswerEvent) {
        if Some(ev.sender.as_ref()) == self.client.user_id() {
            // Our echo, or another of our devices picked up a call ringing here.
            self.stop_ringing_elsewhere(ev.content.call_id.as_str(), "answered on another device")
                .await;
            return;
        }
        let mut active = self.active.lock().await;
        let Some(call) = active.as_mut() else { return };
        if call.call_id != ev.content.call_id.as_str() {
            return;
        }
        let Some(pc) = call.peer.clone() else { return };
        if call.remote_description_set {
            return;
        }
        if let Ok(answer) = RTCSessionDescription::answer(ev.content.answer.sdp) {
            if pc.set_remote_description(answer).await.is_ok() {
                for cand in call.pending_remote_candidates.drain(..) {
                    let _ = pc.add_ice_candidate(cand).await;
                }
                call.remote_description_set = true;
            }
        }
    }

    async fn on_candidates(self: &Arc<Self>, ev: OriginalSyncCallCandidatesEvent) {
        if Some(ev.sender.as_ref()) == self.client.user_id() {
            return;
        }
        let mut active = self.active.lock().await;
        let Some(call) = active.as_mut() else { return };
        if call.call_id != ev.content.call_id.as_str() {
            return;
        }
        for cand in ev.content.candidates {
            // Empty candidate string is the end-of-candidates sentinel.
            if cand.candidate.is_empty() {
                continue;
            }
            let init = RTCIceCandidateInit {
                candidate: cand.candidate,
                sdp_mid: cand.sdp_mid,
                sdp_mline_index: cand.sdp_m_line_index.map(|v| u16::try_from(v).unwrap_or(0)),
                username_fragment: None,
            };
            match (&call.peer, call.remote_description_set) {
                (Some(pc), true) => {
                    let _ = pc.add_ice_candidate(init).await;
                }
                _ => call.pending_remote_candidates.push(init),
            }
        }
    }

    /// The other side hung up or declined (or one of our other devices did).
    async fn on_remote_end(&self, sender: OwnedUserId, call_id: &str, reason: &str) {
        if Some(sender.as_ref()) == self.client.user_id() {
            self.stop_ringing_elsewhere(call_id, "declined on another device").await;
            return;
        }
        let Some(call) = self.take_if(call_id).await else {
            return;
        };
        if let Some(pc) = &call.peer {
            let _ = pc.close().await;
        }
        self.emit(MeshUpdate::CallEnded {
            call_id: call.call_id,
            reason: reason.to_owned(),
        });
    }

    /// Drops an incoming call that is still only ringing here (no media yet).
    async fn stop_ringing_elsewhere(&self, call_id: &str, reason: &str) {
        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|c| c.call_id == call_id && c.peer.is_none())
        {
            *active = None;
            drop(active);
            self.emit(MeshUpdate::CallEnded {
                call_id: call_id.to_owned(),
                reason: reason.to_owned(),
            });
        }
    }

    /// Takes the active call out if it still matches `call_id`.
    async fn take_if(&self, call_id: &str) -> Option<ActiveCall> {
        let mut active = self.active.lock().await;
        if active.as_ref().is_some_and(|c| c.call_id == call_id) {
            active.take()
        } else {
            None
        }
    }

    async fn other_member(&self, room: &matrix_sdk::Room) -> Result<OwnedUserId, String> {
        let me = self.client.user_id();
        let members = room
            .members(matrix_sdk::RoomMemberships::JOIN)
            .await
            .map_err(|e| e.to_string())?;
        members
            .into_iter()
            .map(|m| m.user_id().to_owned())
            .find(|u| Some(u.as_ref()) != me)
            .ok_or_else(|| "no other member in this room to call".to_owned())
    }
}

/// Spawns a task that reads RTP from a remote track, decodes Opus, and feeds
/// the shared playback buffer.
fn spawn_playback_reader(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    buffer: Arc<StdMutex<VecDeque<i16>>>,
    deafened: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut decoder = match opus::Decoder::new(audio::SAMPLE_RATE, opus::Channels::Mono) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("opus decoder: {e}");
                return;
            }
        };
        let mut pcm = vec![0i16; audio::FRAME_SAMPLES * 6];
        loop {
            match track.read_rtp().await {
                Ok((packet, _)) => {
                    if packet.payload.is_empty() {
                        continue;
                    }
                    // Deafened: discard remote audio and keep the buffer empty.
                    if deafened.load(Ordering::Relaxed) {
                        buffer.lock().unwrap().clear();
                        continue;
                    }
                    if let Ok(n) = decoder.decode(&packet.payload, &mut pcm, false) {
                        let mut buf = buffer.lock().unwrap();
                        buf.extend(&pcm[..n]);
                        // Cap latency: drop the oldest if we've buffered > ~1s.
                        while buf.len() > audio::SAMPLE_RATE as usize {
                            buf.pop_front();
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// A short random id for call/party identifiers.
fn new_id() -> String {
    matrix_sdk::ruma::TransactionId::new().to_string()
}

/// True once an invite sent at `sent_ms` is older than its `lifetime_ms`.
fn invite_expired(now_ms: u64, sent_ms: u64, lifetime_ms: u64) -> bool {
    now_ms.saturating_sub(sent_ms) > lifetime_ms
}

#[cfg(test)]
mod tests {
    use super::invite_expired;

    #[test]
    fn invite_expiry() {
        assert!(!invite_expired(10_000, 5_000, 60_000));
        assert!(invite_expired(100_000, 5_000, 60_000));
        // Sender clock ahead of ours: treat as fresh, not expired.
        assert!(!invite_expired(5_000, 10_000, 60_000));
    }
}
