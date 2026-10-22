//! Voice: drives songbird (voice gateway v8, UDP, RTP/Opus, GCM/XChaCha and
//! DAVE E2EE all live inside it) with the endpoint the main gateway hands over,
//! wires the mic in and the decoded audio out, and reports who's speaking.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use async_trait::async_trait;
use songbird::driver::{Channels, CryptoMode, DecodeConfig, DecodeMode, SampleRate};
use songbird::events::context_data::{VoiceData, VoiceTick};
use songbird::events::{Event, EventContext, EventHandler};
use songbird::id::{ChannelId, GuildId, UserId};
use songbird::model::payload::{ClientDisconnect, Speaking};
use songbird::{Config, ConnectionInfo, Driver};
use tokio::sync::{Mutex, mpsc};

use crate::audio;
use crate::gateway::State;

/// A live voice connection: the driver plus the maps the handlers keep.
pub struct Voice {
    driver: Driver,
    shared: Arc<Shared>,
}

struct Shared {
    state: Arc<Mutex<State>>,
    /// ssrc -> user id, learned from speaking updates.
    ssrc: Mutex<HashMap<u32, String>>,
    speaker: async_channel::Sender<Vec<i16>>,
    notify: mpsc::UnboundedSender<()>,
}

impl Voice {
    /// Connects to the channel the gateway's `VoiceServer` event described.
    /// `notify` fires when the speaking set changes, so the snapshot refreshes.
    pub async fn connect(
        state: Arc<Mutex<State>>,
        guild_id: Option<String>,
        channel_id: String,
        endpoint: String,
        token: String,
        session_id: String,
        notify: mpsc::UnboundedSender<()>,
    ) -> anyhow::Result<Voice> {
        let me = state.lock().await.me_id().ok_or_else(|| anyhow::anyhow!("no user id yet"))?;
        // A DM call has no guild; songbird still needs a GuildId slot, so the
        // channel id stands in (Discord accepts it for private calls).
        let gid = guild_id.as_deref().unwrap_or(&channel_id);
        let info = ConnectionInfo {
            channel_id: ChannelId(parse(&channel_id)?),
            endpoint,
            guild_id: GuildId(parse(gid)?),
            session_id,
            token,
            user_id: UserId(parse(&me)?),
        };

        let config = Config::default()
            .crypto_mode(CryptoMode::Aes256Gcm)
            .decode_mode(DecodeMode::Decode(DecodeConfig::new(Channels::Mono, SampleRate::Hz48000)));
        let mut driver = Driver::new(config);

        let speaker = audio::open_speaker()?;
        let shared = Arc::new(Shared { state, ssrc: Mutex::new(HashMap::new()), speaker, notify });

        driver.add_global_event(Event::Core(songbird::CoreEvent::SpeakingStateUpdate), Handler(shared.clone()));
        driver.add_global_event(Event::Core(songbird::CoreEvent::VoiceTick), Handler(shared.clone()));
        driver.add_global_event(Event::Core(songbird::CoreEvent::ClientDisconnect), Handler(shared.clone()));

        driver.connect(info).await.map_err(|e| anyhow::anyhow!("voice connect: {e}"))?;

        // Mic in: songbird resamples the native-rate mono f32 to 48 kHz Opus.
        match audio::open_mic() {
            Ok(mic) => {
                let rate = mic.rate;
                let input = songbird::input::RawAdapter::new(mic, rate, 1);
                driver.play_input(input.into());
            }
            Err(e) => log::warn!("no microphone, listen-only: {e}"),
        }

        {
            let mut s = shared.state.lock().await;
            s.rtc_connected = true;
            s.voice_channel = Some(channel_id);
            s.voice_guild = guild_id;
        }
        Ok(Voice { driver, shared })
    }

    pub fn set_mute(&mut self, mute: bool) {
        self.driver.mute(mute);
    }

    pub async fn leave(mut self) {
        self.driver.leave();
        let mut s = self.shared.state.lock().await;
        s.rtc_connected = false;
        s.speaking.clear();
    }
}

fn parse(id: &str) -> anyhow::Result<NonZeroU64> {
    id.parse::<NonZeroU64>().map_err(|_| anyhow::anyhow!("bad id {id}"))
}

struct Handler(Arc<Shared>);

#[async_trait]
impl EventHandler for Handler {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        match ctx {
            EventContext::SpeakingStateUpdate(Speaking { ssrc, user_id, .. }) => {
                if let Some(uid) = user_id {
                    self.0.ssrc.lock().await.insert(*ssrc, uid.0.to_string());
                }
            }
            EventContext::VoiceTick(VoiceTick { speaking, .. }) => {
                self.0.play_tick(speaking).await;
            }
            EventContext::ClientDisconnect(ClientDisconnect { user_id }) => {
                let uid = user_id.0.to_string();
                self.0.ssrc.lock().await.retain(|_, v| *v != uid);
                let mut s = self.0.state.lock().await;
                if s.speaking.remove(&uid).is_some() {
                    let _ = self.0.notify.send(());
                }
            }
            _ => {}
        }
        None
    }
}

impl Shared {
    async fn play_tick(&self, speaking: &HashMap<u32, VoiceData>) {
        let ssrc = self.ssrc.lock().await;
        let mut now: HashMap<String, bool> = HashMap::new();
        for (s, data) in speaking {
            if let Some(pcm) = &data.decoded_voice {
                let _ = self.speaker.try_send(pcm.clone());
            }
            if let Some(uid) = ssrc.get(s) {
                now.insert(uid.clone(), true);
            }
        }
        drop(ssrc);
        let mut state = self.state.lock().await;
        if state.speaking != now {
            state.speaking = now;
            let _ = self.notify.send(());
        }
    }
}
