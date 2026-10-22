//! One 1:1 audio call at a time, reported to Mesh as `waCall`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use whatsapp_rust::prelude::*;
use whatsapp_rust::voip::{CallEvent, CallHandle};
use whatsapp_rust::wacore::types::call::IncomingCall;
use whatsapp_rust::wacore::voip_control::signaling::CallPhase;

use crate::audio::{spawn_mic, spawn_speaker};

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

/// Places a call; reports `Started` then phases.
pub fn dial(client: Arc<Client>, peer: Jid, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            client.voip().call(&peer).audio(mic, speaker).start().await.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        report(started, seq, true, tx);
    });
}

/// Answers a ringing call with the mic and speaker.
pub fn answer(client: Arc<Client>, call: Box<IncomingCall>, seq: u64, tx: UnboundedSender<Update>) {
    tokio::spawn(async move {
        let started = async {
            let (mic, speaker) = (spawn_mic()?, spawn_speaker()?);
            client.voip().accept(&call).audio(mic, speaker).start().await.map_err(|e| anyhow::anyhow!("{e}"))
        }
        .await;
        if started.is_err() {
            let _ = client.voip().reject(&call).await;
        }
        report(started, seq, false, tx);
    });
}

fn report(started: anyhow::Result<CallHandle>, seq: u64, outgoing: bool, tx: UnboundedSender<Update>) {
    match started {
        Ok(handle) => {
            let handle = Arc::new(handle);
            let _ = tx.send(Update::Started(seq, Ok(handle.clone())));
            tokio::spawn(watch(handle, seq, outgoing, tx));
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
