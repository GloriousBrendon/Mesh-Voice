// Runs inside the hidden Discord web client. Reads Discord's own stores and
// calls its own actions, so Mesh can show and drive voice without the window.
// Talks back through the `meshEmit` DevTools binding.
(() => {
  if (window.__mesh) return "already";

  // Discord loads several webpack runtimes into one chunk global; the live
  // app is the one with the most instantiated modules.
  const runtime = () => {
    const chunks = window.webpackChunkdiscord_app;
    if (!chunks) return null;
    const runtimes = [];
    chunks.push([[Symbol()], {}, (r) => { runtimes.push(r); }]);
    chunks.pop();
    const size = (r) => Object.keys(r.c || {}).length;
    return runtimes.reduce((a, b) => (size(b) > size(a) ? b : a), runtimes[0]);
  };

  // i18n proxies answer every property; skip them.
  const real = (c) => {
    try { return c[`__mesh_${Math.random()}`] === undefined; } catch { return false; }
  };
  const candidates = function* (req) {
    for (const id in req.c) {
      const e = req.c[id].exports;
      if (!e) continue;
      for (const c of [e, ...Object.values(e)]) {
        if (c && (typeof c === "object" || typeof c === "function") && real(c)) yield c;
      }
    }
  };

  let found = null;
  const find = () => {
    const req = runtime();
    if (!req || Object.keys(req.c || {}).length < 1000) return null;
    const stores = {};
    const wanted = ["UserStore", "GuildStore", "GuildChannelStore", "GuildMemberStore", "ChannelStore",
      "VoiceStateStore", "SpeakingStore", "MediaEngineStore", "SelectedChannelStore", "RTCConnectionStore"];
    let channel, audio, call;
    for (const c of candidates(req)) {
      try {
        if (typeof c.getName === "function") {
          const name = c.getName();
          if (wanted.includes(name)) stores[name] = c;
        }
        if (!channel && typeof c.selectVoiceChannel === "function" && typeof c.disconnect === "function") channel = c;
        if (!audio && typeof c.toggleSelfMute === "function" && typeof c.toggleSelfDeaf === "function") audio = c;
        if (!call && typeof c.call === "function" && typeof c.ring === "function") call = c;
      } catch {}
    }
    const missing = wanted.filter((n) => !stores[n]);
    if (missing.length || !channel || !audio || !call) return null;
    return { s: stores, channel, audio, call, dispatcher: stores.VoiceStateStore._dispatcher };
  };

  const user = (id) => {
    const u = found.s.UserStore.getUser(id);
    return {
      id,
      name: (u && (u.globalName || u.username)) || id,
      avatar: u && u.avatar ? `https://cdn.discordapp.com/avatars/${id}/${u.avatar}.png?size=64` : null,
    };
  };

  const members = (channelId, guildId) =>
    Object.values(found.s.VoiceStateStore.getVoiceStatesForChannel(channelId) || {}).map((vs) => {
      const u = user(vs.userId);
      const nick = guildId ? found.s.GuildMemberStore.getNick(guildId, vs.userId) : null;
      return {
        ...u,
        name: nick || u.name,
        speaking: !!found.s.SpeakingStore.isSpeaking(vs.userId),
        mute: !!(vs.selfMute || vs.mute),
        deaf: !!(vs.selfDeaf || vs.deaf),
      };
    });

  const snapshot = () => {
    const s = found.s;
    const current = s.SelectedChannelStore.getVoiceChannelId() || null;
    const currentChannel = current ? s.ChannelStore.getChannel(current) : null;
    const guilds = Object.values(s.GuildStore.getGuilds()).map((g) => ({
      id: g.id,
      name: g.name,
      icon: g.icon ? `https://cdn.discordapp.com/icons/${g.id}/${g.icon}.png?size=64` : null,
      voice_channels: ((s.GuildChannelStore.getChannels(g.id) || {}).VOCAL || []).map(({ channel }) => ({
        id: channel.id,
        name: channel.name,
        members: members(channel.id, g.id),
      })),
    }));
    const me = s.UserStore.getCurrentUser();
    return {
      me: me ? user(me.id) : null,
      voice: {
        channel_id: current,
        guild_id: (currentChannel && currentChannel.guild_id) || null,
        channel_name: currentChannel ? currentChannel.name || "call" : null,
        connection: String((s.RTCConnectionStore.getState && s.RTCConnectionStore.getState()) || ""),
        self_mute: !!s.MediaEngineStore.isSelfMute(),
        self_deaf: !!s.MediaEngineStore.isSelfDeaf(),
        members: current ? members(current, currentChannel && currentChannel.guild_id) : [],
      },
      guilds,
    };
  };

  let pending = null;
  const emit = () => {
    if (pending) return;
    pending = setTimeout(() => {
      pending = null;
      try { window.meshEmit(JSON.stringify(snapshot())); } catch (e) { console.warn("mesh emit", e); }
    }, 120);
  };

  const events = ["VOICE_STATE_UPDATES", "SPEAKING", "VOICE_CHANNEL_SELECT", "AUDIO_TOGGLE_SELF_MUTE",
    "AUDIO_TOGGLE_SELF_DEAF", "RTC_CONNECTION_STATE", "CONNECTION_OPEN", "GUILD_CREATE", "GUILD_DELETE",
    "CHANNEL_CREATE", "CHANNEL_DELETE", "CALL_CREATE", "CALL_UPDATE", "CALL_DELETE"];

  window.__mesh = {
    ready: () => !!found,
    snapshot: () => (found ? JSON.stringify(snapshot()) : null),
    // Guild voice channels: join. DM / group DM: start or join the call (rings).
    join: (channelId) => {
      const ch = found.s.ChannelStore.getChannel(channelId);
      if (ch && !ch.guild_id) {
        const recipient = (ch.recipients || [])[0];
        found.call.call(channelId, false, true, recipient);
      } else {
        found.channel.selectVoiceChannel(channelId);
      }
    },
    leave: () => found.channel.disconnect(),
    toggleMute: () => found.audio.toggleSelfMute(),
    toggleDeaf: () => found.audio.toggleSelfDeaf(),
  };

  // Stores fill in after login/startup; keep looking until they're there.
  const boot = () => {
    found = find();
    if (!found) return setTimeout(boot, 1000);
    for (const e of events) found.dispatcher.subscribe(e, emit);
    emit();
  };
  boot();
  return "installed";
})();
