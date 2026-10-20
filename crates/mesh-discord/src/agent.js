// Runs inside the hidden Discord web client. Reads Discord's own stores and
// calls its own actions, so Mesh can show and drive voice without the window.
// Talks back through the `meshEmit` DevTools binding.
(() => {
  // Bump when the API below changes, so a running page takes the new agent.
  const version = 6;
  if (window.__mesh && window.__mesh.version >= version) return "already";

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
    // Optional: voice keeps working if Discord renames these.
    const extra = ["MessageStore", "ReadStateStore"];
    let channel, audio, call, invite, messages;
    for (const c of candidates(req)) {
      try {
        if (typeof c.getName === "function") {
          const name = c.getName();
          if (wanted.includes(name) || extra.includes(name)) stores[name] = c;
        }
        if (!channel && typeof c.selectVoiceChannel === "function" && typeof c.disconnect === "function") channel = c;
        if (!audio && typeof c.toggleSelfMute === "function" && typeof c.toggleSelfDeaf === "function") audio = c;
        if (!call && typeof c.call === "function" && typeof c.ring === "function") call = c;
        if (!invite && typeof c.acceptInvite === "function" && typeof c.resolveInvite === "function") invite = c;
        if (!messages && typeof c.sendMessage === "function" && typeof c.fetchMessages === "function") messages = c;
      } catch {}
    }
    const missing = wanted.filter((n) => !stores[n]);
    if (missing.length || !channel || !audio || !call) return null;
    return { s: stores, channel, audio, call, invite, messages, dispatcher: stores.VoiceStateStore._dispatcher };
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
    const guilds = Object.values(s.GuildStore.getGuilds()).map((g) => {
      const channels = s.GuildChannelStore.getChannels(g.id) || {};
      const category = (channel) => {
        const parent = channel.parent_id && s.ChannelStore.getChannel(channel.parent_id);
        return (parent && parent.name) || null;
      };
      return {
        id: g.id,
        name: g.name,
        icon: g.icon ? `https://cdn.discordapp.com/icons/${g.id}/${g.icon}.png?size=64` : null,
        // SELECTABLE: the text-like channels you can see, in Discord's order.
        text_channels: (channels.SELECTABLE || []).map(({ channel }) => ({
          id: channel.id,
          name: channel.name,
          category: category(channel),
        })),
        voice_channels: (channels.VOCAL || []).map(({ channel }) => ({
          id: channel.id,
          name: channel.name,
          category: category(channel),
          members: members(channel.id, g.id),
        })),
      };
    });
    const reads = s.ReadStateStore;
    const dms = s.ChannelStore.getSortedPrivateChannels().map((ch) => {
      const recipients = (ch.recipients || []).map(user);
      return {
        id: ch.id,
        name: ch.name || recipients.map((r) => r.name).join(", ") || "Direct message",
        recipient_id: recipients.length === 1 ? recipients[0].id : null,
        // A snowflake id carries its time: ms since Discord's 2015 epoch, << 22.
        last_ms: ch.lastMessageId ? Number(BigInt(ch.lastMessageId) >> 22n) + 1420070400000 : 0,
        unread: !!(reads && reads.hasUnread(ch.id)),
        mentions: (reads && reads.getMentionCount(ch.id)) || 0,
      };
    });
    const me = s.UserStore.getCurrentUser();
    return {
      me: me ? user(me.id) : null,
      dms,
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

  // The channel Mesh shows, whose messages stream through `meshMessages`.
  let open = null;
  let pendingMessages = null;
  const emitMessages = () => {
    if (!open || pendingMessages) return;
    pendingMessages = setTimeout(() => {
      pendingMessages = null;
      try {
        const collection = found.s.MessageStore.getMessages(open);
        const list = collection.toArray ? collection.toArray() : collection._array || [];
        window.meshMessages(JSON.stringify({ channel_id: open, messages: list.map(message) }));
      } catch (e) { console.warn("mesh messages", e); }
    }, 80);
  };
  // Discord message types Mesh draws as a plain message; the rest are system lines.
  const TEXT = [0, 19, 20, 21];
  const SYSTEM = { 1: "added someone", 2: "removed someone", 3: "started a call", 6: "pinned a message", 7: "joined the server" };
  const message = (m) => {
    const author = user(m.author.id);
    const text = TEXT.includes(m.type);
    return {
      id: m.id,
      author_id: m.author.id,
      author: m.nick || author.name,
      content: text ? m.content : SYSTEM[m.type] || "did something Mesh can't show yet",
      system: !text,
      timestamp_ms: m.timestamp ? m.timestamp.valueOf() : 0,
      attachments: (m.attachments || []).map((a) => ({ filename: a.filename, url: a.url })),
    };
  };

  const events = ["VOICE_STATE_UPDATES", "SPEAKING", "VOICE_CHANNEL_SELECT", "AUDIO_TOGGLE_SELF_MUTE",
    "AUDIO_TOGGLE_SELF_DEAF", "RTC_CONNECTION_STATE", "CONNECTION_OPEN", "GUILD_CREATE", "GUILD_DELETE",
    "CHANNEL_CREATE", "CHANNEL_DELETE", "CALL_CREATE", "CALL_UPDATE", "CALL_DELETE",
    // DM order and unread state.
    "MESSAGE_CREATE", "MESSAGE_ACK"];
  const messageEvents = ["MESSAGE_CREATE", "MESSAGE_UPDATE", "MESSAGE_DELETE", "MESSAGE_DELETE_BULK",
    "LOAD_MESSAGES_SUCCESS", "MESSAGE_SEND_FAILED"];

  // Tells Mesh about new DMs and mentions of us (never our own messages), for
  // a desktop notification.
  const notify = ({ message: m }) => {
    const me = found.s.UserStore.getCurrentUser();
    if (!m || !m.author || !me || m.author.id === me.id) return;
    const ch = found.s.ChannelStore.getChannel(m.channel_id);
    if (!ch) return;
    if (ch.guild_id && !(m.mentions || []).some((u) => u.id === me.id)) return;
    const where = ch.guild_id ? ` in #${ch.name}` : ch.name ? ` in ${ch.name}` : "";
    const title = (m.author.global_name || m.author.username) + where;
    window.meshNotify(JSON.stringify({ channel_id: m.channel_id, title, body: m.content || "sent an attachment" }));
  };

  window.__mesh = {
    version,
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
    // Shows a text channel or DM in Mesh: loads its latest messages and
    // streams them (and every change) back; `null` stops.
    open: (channelId) => {
      open = channelId;
      if (!channelId || !found.messages) return;
      found.messages.fetchMessages({ channelId, limit: 50 });
      emitMessages();
    },
    send: (channelId, content) =>
      found.messages.sendMessage(channelId, { content, tts: false, invalidEmojis: [], validNonShortcutEmojis: [] }, true, {}),
    // Joins a server from a discord.gg / discord.com/invite link or a bare
    // code. Discord's own client asks for a captcha in its window if it wants
    // one; "waiting" tells Mesh to bring that window up.
    acceptInvite: (link) => {
      const code = String(link).trim()
        .replace(/^(https?:\/\/)?(www\.)?(discord\.gg|discord(app)?\.com\/invite)\//i, "")
        .split(/[/?#\s]/)[0];
      const report = (r) => window.meshInvite(JSON.stringify(r));
      if (!found || !found.invite) return report({ state: "failed", error: "Discord isn't ready yet" });
      if (!code) return report({ state: "failed", error: "That isn't an invite link" });
      let settled = false;
      setTimeout(() => settled || report({ state: "waiting" }), 2500);
      found.invite
        .acceptInvite({ inviteKey: code, context: { location: "Join Guild Modal" }, skipOnboarding: true })
        .then(
          (r) => { settled = true; report({ state: "joined", guild: (r && r.guild && r.guild.name) || null }); },
          (e) => {
            settled = true;
            const error = (e && ((e.body && e.body.message) || e.message)) || "Discord refused the invite";
            report({ state: "failed", error });
          },
        );
    },
  };

  // Stores fill in after login/startup; keep looking until they're there.
  const boot = () => {
    found = find();
    if (!found) return setTimeout(boot, 1000);
    for (const e of events) found.dispatcher.subscribe(e, emit);
    for (const e of messageEvents) found.dispatcher.subscribe(e, emitMessages);
    found.dispatcher.subscribe("MESSAGE_CREATE", notify);
    emit();
  };
  boot();
  return "installed";
})();
