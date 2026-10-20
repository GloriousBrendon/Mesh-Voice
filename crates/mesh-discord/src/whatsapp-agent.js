// Runs inside WhatsApp Web (a tab of the engine browser). Reads WhatsApp's
// own collections (Haste modules, required by name) and calls its own
// actions, so Mesh can show and send WhatsApp chats without the window.
// Reports through the waEmit / waMessages / waNotify / waMedia / waSent /
// waFile / waForwarded / waFrames / waPcm / waContacts / waChatStarted /
// waChatAction bindings.
// See design/WHATSAPP-NATIVE.md for the modules used.
(() => {
  // Bump when the API below changes, so a running page takes the new agent.
  const version = 14;
  if (window.__mesh && window.__mesh.version >= version) return "already";

  const req = (name) => {
    try { return window.require(name); } catch { return null; }
  };
  let wa = null;
  const me = () => {
    const prefs = req("WAWebUserPrefsMeUser");
    return prefs && prefs.getMaybeMePnUser();
  };
  // Logged in and the stores are there.
  const find = () => {
    const chats = req("WAWebChatCollection"), msgs = req("WAWebMsgCollection"), send = req("WAWebSendTextMsgChatAction");
    if (!me() || !chats || !msgs || !send) return null;
    return {
      Chat: chats.ChatCollection,
      Msg: msgs.MsgCollection,
      Pics: (req("WAWebProfilePicThumbCollection") || {}).ProfilePicThumbCollection,
      send,
      downloads: (req("WAWebDownloadManager") || {}).downloadManager,
      attachColl: req("WAWebAttachMediaCollection"),
      fileType: (req("WAWebFileUtils") || {}).FILETYPE,
      pickerOrigin: (req("WAWebWamEnumMediaPickerOriginType") || {}).MEDIA_PICKER_ORIGIN_TYPE,
      Reactions: (req("WAWebReactionsCollection") || {}).ReactionsCollection,
      quotes: req("WAWebQuotedMsgModelUtils"),
    };
  };

  const pic = (id) => {
    const p = wa.Pics && id && wa.Pics.get(id);
    return (p && p.eurl) || null;
  };
  const muted = (chat) => {
    const until = chat.muteExpiration || 0;
    return until < 0 || until * 1000 > Date.now();
  };
  const title = (chat) => chat.formattedTitle || chat.name || String(chat.id.user || chat.id);
  const isGroup = (id) => String(id).endsWith("@g.us");

  // Profile pictures load lazily; ask once each for the chats on show.
  const asked = new Set();
  const askPictures = (chats) => {
    for (const c of chats) {
      const id = String(c.id);
      if (!wa.Pics || asked.has(id) || pic(c.id)) continue;
      asked.add(id);
      Promise.resolve(wa.Pics.find(c.id)).then(emit, () => {});
    }
  };

  const CHATS = 100;
  const toChat = (c) => ({
    id: String(c.id),
    name: title(c),
    group: isGroup(c.id),
    unread: Math.max(0, c.unreadCount || 0),
    last_ms: (c.t || 0) * 1000,
    muted: muted(c),
    avatar: pic(c.id),
  });
  const snapshot = () => {
    const recent = wa.Chat.getModelsArray().sort((a, b) => (b.t || 0) - (a.t || 0));
    const chats = recent.filter((c) => !c.archive).slice(0, CHATS);
    // Archived chats stay out of the list; Mesh shows them on request.
    const archived = recent.filter((c) => c.archive).slice(0, CHATS);
    askPictures(chats.slice(0, 40));
    return { me: String(me()), chats: chats.map(toChat), archived: archived.map(toChat) };
  };

  let pending = null;
  const emit = () => {
    if (pending || !wa) return;
    pending = setTimeout(() => {
      pending = null;
      try { window.waEmit(JSON.stringify(snapshot())); } catch (e) { console.warn("mesh wa emit", e); }
    }, 150);
  };

  // Downloadable attachments: Mesh shows a button that opens them in the OS's
  // player/viewer (fetched on click through `saveMedia`).
  const FILE = { video: "Video", ptt: "Voice note", audio: "Audio", document: "Document" };
  // Other non-text kinds Mesh just labels.
  const LABEL = { location: "Location", vcard: "Contact" };
  const SYSTEM = { revoked: "deleted a message", call_log: "called", ciphertext: "sent a message WhatsApp is still decrypting" };
  // Group events (type gp2), by subtype; others show as a generic update.
  const GROUP = {
    add: "added someone", remove: "removed someone", leave: "left", invite: "joined via an invite link",
    create: "created the group", subject: "changed the group name", picture: "changed the group icon",
    description: "changed the group description", promote: "made someone an admin", demote: "removed an admin",
  };
  // A sensible filename for a media message (keeps a document's own name).
  const fileName = (m) => {
    if (m.type === "document" && m.filename) return m.filename;
    const ext = ((m.mimetype || "").split(";")[0].split("/")[1] || "bin").toLowerCase();
    return `${(FILE[m.type] || "file").toLowerCase().replace(/\s+/g, "-")}.${ext}`;
  };
  // What a reply quotes: its author and a line of its text (or its kind).
  const KIND = { image: "Photo", sticker: "Sticker", ...FILE, ...LABEL };
  const quoteOf = (m) => {
    if (!m.quotedStanzaID || !m.quotedMsg) return null;
    let q = null;
    try { q = wa.quotes && wa.quotes.getQuotedMsgObj(m); } catch {}
    let mine = false;
    try { mine = !!(wa.quotes && wa.quotes.isMyQuotedMsg(m)); } catch {}
    const s = q && q.senderObj;
    const author = mine ? "You" : (s && (s.name || s.pushname || s.verifiedName)) || String(m.quotedParticipant || "").split("@")[0];
    const d = m.quotedMsg;
    const text = d.body && d.type === "chat" ? d.body : d.caption || (KIND[d.type] ? KIND[d.type] : d.body || "");
    return { author, text: text.slice(0, 200) };
  };
  // Reaction pills: one per emoji, with whether one of them is ours.
  // They load lazily: fetch a message's once, then re-send the chat.
  const loadingReactions = new Set();
  const reactionsOf = (m) => {
    if (!m.hasReaction || !wa.Reactions) return [];
    const r = wa.Reactions.get(m.id);
    if (!r) {
      const id = String(m.id);
      if (!loadingReactions.has(id)) {
        loadingReactions.add(id);
        Promise.resolve(wa.Reactions.find(m.id)).then(emitMessages, () => {});
      }
      return [];
    }
    if (!r.reactions) return [];
    return r.reactions.getModelsArray()
      .filter((x) => x.aggregateEmoji && x.senders && x.senders.length)
      .map((x) => ({ key: x.aggregateEmoji, count: x.senders.length, mine: !!x.hasReactionByMe }));
  };
  const message = (m) => {
    const fromMe = !!m.id.fromMe;
    const sender = m.senderObj;
    const authorId = String(fromMe ? me() : m.author || m.from);
    const author = fromMe ? "You" : (sender && (sender.name || sender.pushname || sender.verifiedName)) || authorId.split("@")[0];
    let content, system = false, image = null, file = null;
    if (m.type === "chat") content = m.body || "";
    else if (m.type === "image") {
      content = m.caption || "";
      image = { width: m.width || null, height: m.height || null, sticker: false };
    } else if (m.type === "sticker") {
      content = "";
      image = { width: m.width || 512, height: m.height || 512, sticker: true };
    } else if (m.type === "video" && m.isGif) {
      // A GIF is a short silent mp4: Mesh loops its frames inline.
      content = m.caption || "";
      image = { width: m.width || null, height: m.height || null, sticker: false, gif: true };
    } else if (FILE[m.type]) {
      content = m.caption || "";
      file = {
        name: fileName(m), kind: FILE[m.type], size: m.size || null,
        duration: Number(m.duration) || null,
        waveform: m.type === "ptt" && m.waveform ? Array.from(m.waveform) : [],
        // A video's body is its small JPEG preview (base64): Mesh's poster.
        thumb: m.type === "video" && /^\/9j\//.test(m.body || "") ? m.body : null,
        width: m.width || null,
        height: m.height || null,
      };
    } else if (LABEL[m.type]) content = `[${LABEL[m.type]}]`;
    else if (SYSTEM[m.type]) { content = SYSTEM[m.type]; system = true; }
    // Business messages with buttons: show their text.
    else if (m.type === "interactive" && (m.caption || m.body)) content = m.caption || m.body;
    else if (m.type === "gp2") { content = GROUP[m.subtype] || "updated the group"; system = true; }
    else return null; // protocol, encryption notices, group events…
    return {
      id: String(m.id),
      author_id: authorId,
      author,
      avatar: pic(fromMe ? me() : m.author || m.from),
      content,
      system,
      timestamp_ms: (m.t || 0) * 1000,
      image,
      file,
      quote: quoteOf(m),
      reactions: reactionsOf(m),
    };
  };

  // The chat Mesh shows, whose messages stream through `waMessages`.
  let open = null;
  let pendingMessages = null;
  const emitMessages = () => {
    if (!open || pendingMessages) return;
    pendingMessages = setTimeout(() => {
      pendingMessages = null;
      const chat = wa.Chat.get(open);
      if (!chat) return;
      const list = chat.msgs.getModelsArray().slice(-50).map(message).filter(Boolean);
      try { window.waMessages(JSON.stringify({ chat_id: open, messages: list })); } catch (e) { console.warn("mesh wa messages", e); }
    }, 80);
  };

  // Marks a chat read: sends the receipt/sync and clears the local count
  // (which fires change:unreadCount → a fresh snapshot for Mesh's badge).
  const markSeen = (chat) => {
    if (!chat || (chat.unreadCount || 0) <= 0) return;
    const seen = req("WAWebChatSendConversationSeen");
    const msgs = chat.msgs.getModelsArray();
    const key = msgs.length ? msgs[msgs.length - 1].id : undefined;
    if (seen) Promise.resolve(seen.sendConversationSeen({ chat, key, unreadDelta: -1 })).catch(() => {});
    try { chat.unreadCount = 0; chat.markedUnread = false; } catch {}
    emit(); // push a fresh snapshot so Mesh clears the badge now
  };

  const onMessage = (m) => {
    const chatId = String(m.id.remote);
    if (chatId === open) {
      emitMessages();
      // The chat is on screen in Mesh, so a new message from someone else is
      // seen the moment it lands (WhatsApp Web does the same when focused).
      if (m.isNewMsg && !m.id.fromMe) markSeen(wa.Chat.get(m.id.remote));
    }
    emit();
    // A desktop notification for new messages from others in unmuted chats.
    if (!m.isNewMsg || m.id.fromMe) return;
    const chat = wa.Chat.get(m.id.remote);
    const shown = message(m);
    if (!chat || muted(chat) || !shown || shown.system) return;
    const name = isGroup(chatId) ? `${shown.author} in ${title(chat)}` : shown.author;
    const body = shown.content || (shown.image ? "Photo" : shown.file ? shown.file.kind : "");
    try { window.waNotify(JSON.stringify({ chat_id: chatId, title: name, body })); } catch {}
  };

  // Shrinks a picture so Mesh gets a few KB: photos to `width` px (480 in the
  // chat, 1600 full screen) as JPEG, stickers to 160 px as PNG (keeping their
  // transparency). ponytail: an animated sticker shows its first frame.
  const shrink = async (blob, sticker, width = 480) => {
    const bitmap = await createImageBitmap(blob);
    const scale = Math.min(1, (sticker ? 160 : width) / bitmap.width);
    const canvas = new OffscreenCanvas(Math.round(bitmap.width * scale), Math.round(bitmap.height * scale));
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    return sticker ? canvas.convertToBlob({ type: "image/png" }) : canvas.convertToBlob({ type: "image/jpeg", quality: 0.8 });
  };
  const decrypt = async (id) => {
    const m = wa.Msg.get(id);
    if (!m || !wa.downloads) throw new Error("no such message");
    const buf = await wa.downloads.downloadAndMaybeDecrypt({
      directPath: m.directPath, encFilehash: m.encFilehash, filehash: m.filehash,
      mediaKey: m.mediaKey, mediaKeyTimestamp: m.mediaKeyTimestamp, type: m.type, mimetype: m.mimetype,
      signal: new AbortController().signal,
      // WhatsApp's performance logger; nothing to log here.
      downloadQpl: new Proxy({}, { get: () => () => {} }),
    });
    return new Blob([buf], { type: m.mimetype || "image/jpeg" });
  };
  const b64 = async (blob) => {
    const bytes = new Uint8Array(await blob.arrayBuffer());
    let binary = "";
    for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(binary);
  };
  // A GIF's frames (it's an mp4): up to 6 s at 12 fps, 360 px wide JPEGs.
  // Seeking is slow in a background window (~0.75 s each), so several video
  // elements each grab a share of the frames at once.
  const FPS = 12, SEEKERS = 6;
  const gifFrames = async (id) => {
    const src = URL.createObjectURL(await decrypt(id));
    const load = () => new Promise((ok, no) => {
      const v = document.createElement("video");
      v.muted = true;
      v.onloadeddata = () => ok(v);
      v.onerror = () => no(new Error("can't decode the GIF"));
      v.src = src;
    });
    try {
      const first = await load();
      const count = Math.max(1, Math.min(6 * FPS, Math.round((first.duration || 1) * FPS)));
      const scale = Math.min(1, 360 / (first.videoWidth || 360));
      const [w, h] = [Math.round(first.videoWidth * scale) || 1, Math.round(first.videoHeight * scale) || 1];
      const frames = new Array(count);
      const seeker = async (v, k) => {
        const canvas = new OffscreenCanvas(w, h), ctx = canvas.getContext("2d");
        for (let i = k; i < count; i += SEEKERS) {
          if (i > 0) {
            // Don't hang on a frame whose `seeked` never fires.
            const seeked = new Promise((ok) => { v.onseeked = ok; setTimeout(ok, 1500); });
            v.currentTime = i / FPS;
            await seeked;
          }
          ctx.drawImage(v, 0, 0, w, h);
          frames[i] = await b64(await canvas.convertToBlob({ type: "image/jpeg", quality: 0.75 }));
        }
      };
      const videos = [first, ...(await Promise.all([...Array(Math.min(SEEKERS, count) - 1)].map(load)))];
      await Promise.all(videos.map(seeker));
      return frames;
    } finally {
      URL.revokeObjectURL(src);
    }
  };

  const setArchived = async (chatId, archive) => {
    const report = (ok, error) => window.waChatAction(JSON.stringify({ chat_id: chatId, ok, error: error || null }));
    try {
      const chat = wa.Chat.get(chatId), act = req("WAWebSetArchiveChatAction");
      if (!chat || !act) throw new Error("couldn't find that chat");
      if (!!chat.archive !== archive) await act.setArchive(chat, archive, false);
      report(true);
    } catch (e) {
      report(false, String((e && e.message) || e));
    }
  };

  window.__mesh = {
    version,
    ready: () => !!wa,
    snapshot: () => (wa ? JSON.stringify(snapshot()) : null),
    open: (chatId) => {
      open = chatId || null;
      emitMessages();
      // Opening a chat marks it read (clears unread here and on the phone,
      // and sends read receipts), as WhatsApp Web does on view.
      if (chatId) markSeen(wa.Chat.get(chatId));
    },
    // Sends text; `quotedId` (a message id) makes it a reply to that message.
    send: (chatId, text, quotedId) => {
      const chat = wa.Chat.get(chatId);
      const quotedMsg = quotedId ? wa.Msg.get(quotedId) : undefined;
      if (chat) return wa.send.sendTextMsgToChat(chat, text, quotedMsg ? { quotedMsg } : {});
    },
    // Address-book contacts whose name or number matches, through waContacts.
    searchContacts: (query) => {
      // A national number's leading 0 stands in for the country code.
      const q = (query || "").trim().toLowerCase(), digits = q.replace(/\D/g, "").replace(/^0/, "");
      const C = (req("WAWebContactCollection") || {}).ContactCollection;
      const contacts = !q || !C ? [] : C.getModelsArray()
        .filter((c) => c.name && String(c.id).endsWith("@c.us"))
        .filter((c) => c.name.toLowerCase().includes(q) || (digits.length >= 3 && String(c.id.user).includes(digits)))
        .slice(0, 20)
        .map((c) => ({ id: String(c.id), name: c.name, number: String(c.id.user) }));
      window.waContacts(JSON.stringify({ query, contacts }));
    },
    // Opens (or makes) the chat with a contact id or a phone number; answers
    // through waChatStarted with the chat, so Mesh can show it.
    startChat: async (input) => {
      const report = (r) => window.waChatStarted(JSON.stringify(r));
      try {
        const Wid = req("WAWebWidFactory"), Find = req("WAWebFindChatAction"), Exists = req("WAWebQueryExistsJob");
        let wid;
        if (input.includes("@")) wid = Wid.createWid(input);
        else {
          let digits = input.replace(/\D/g, "");
          // A national number (0…) takes our own country code.
          // ponytail: assumes it's as long as ours; type +code otherwise.
          const mine = String(me()).split("@")[0];
          if (digits.startsWith("0")) digits = mine.slice(0, Math.max(0, mine.length - digits.length + 1)) + digits.slice(1);
          if (digits.length < 8) throw new Error("that doesn't look like a phone number");
          const found = await Exists.queryWidExists(Wid.createWid(digits + "@c.us"));
          if (!found || !found.wid) throw new Error(`+${digits} isn't on WhatsApp`);
          wid = found.wid;
        }
        const res = await Find.findOrCreateLatestChat(wid);
        const chat = (res && res.chat) || res;
        if (!chat || !chat.id) throw new Error("couldn't open that chat");
        report({ chat: toChat(chat) });
      } catch (e) {
        report({ error: String((e && e.message) || e) });
      }
    },
    archive: (chatId) => setArchived(chatId, true),
    unarchive: (chatId) => setArchived(chatId, false),
    // Deletes a chat for us (as WhatsApp's "Delete chat" does).
    deleteChat: async (chatId) => {
      const report = (ok, error) => window.waChatAction(JSON.stringify({ chat_id: chatId, ok, error: error || null }));
      try {
        const chat = wa.Chat.get(chatId), act = req("WAWebDeleteChatAction");
        if (!chat || !act) throw new Error("couldn't find that chat");
        const ok = await act.sendDelete(chat);
        if (ok === false) throw new Error(isGroup(chat.id) ? "WhatsApp wouldn't delete it (leave the group first)" : "WhatsApp wouldn't delete it");
        report(true);
      } catch (e) {
        report(false, String((e && e.message) || e));
      }
    },
    // Reacts to a message with `emoji`; "" takes our reaction back.
    react: async (msgId, emoji) => {
      const m = wa.Msg.get(msgId);
      const act = req("WAWebSendReactionMsgAction");
      if (!m || !act) return;
      try { await act.sendReactionToMsg(m, emoji); } catch (e) { console.warn("mesh wa react", e); }
      emitMessages();
    },
    // Forwards a message to another chat, keeping any caption.
    forward: async (msgId, toChatId) => {
      const report = (ok, error) => window.waForwarded(JSON.stringify({ chat_id: toChatId, ok, error: error || null }));
      try {
        const m = wa.Msg.get(msgId), chat = wa.Chat.get(toChatId);
        const act = req("WAWebForwardMessagesToChat");
        if (!m || !chat || !act) throw new Error("couldn't find that message or chat");
        await act.forwardMessagesToChats({ msgs: [m], chats: [chat], includeCaption: true });
        report(true);
      } catch (e) {
        report(false, String((e && e.message) || e));
      }
    },
    // Sends a file (base64) to a chat through WhatsApp Web's own attach flow:
    // images go as photos, everything else as a document. `caption` rides along.
    sendFile: async (chatId, filename, mimetype, base64, caption) => {
      const report = (ok, error) => window.waSent(JSON.stringify({ chat_id: chatId, ok, error: error || null }));
      try {
        const chat = wa.Chat.get(chatId);
        if (!chat || !wa.attachColl) throw new Error("WhatsApp isn't ready for attachments");
        const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
        const file = new File([bytes], filename, { type: mimetype });
        const type = /^image\//.test(mimetype) ? wa.fileType.IMAGE : /^video\//.test(mimetype) ? wa.fileType.VIDEO : wa.fileType.DOCUMENT;
        const coll = new wa.attachColl({ chatParticipantCount: chat.getParticipantCount ? chat.getParticipantCount() : 1 });
        await coll.processAttachmentsForChat([{ file, type, filename, mimetype }], wa.pickerOrigin.GALLERY, chat);
        const medias = coll.getValidMedias();
        for (const m of medias) {
          // Caption rides in the media's own field (ignored if the build differs).
          if (caption) try { m.caption = caption; } catch {}
          await m.sendToChat({ chat, options: { type: m.type, caption: caption || undefined } });
        }
        report(true);
      } catch (e) {
        report(false, String((e && e.message) || e));
      }
    },
    // Decrypts a video / voice note / document in full (no shrink) and hands
    // the raw bytes back through waFile, so Mesh can open it in the OS player.
    saveMedia: async (id) => {
      const report = (r) => window.waFile(JSON.stringify({ id, ...r }));
      try {
        const m = wa.Msg.get(id);
        if (!m) throw new Error("no such message");
        report({ filename: fileName(m), data: await b64(await decrypt(id)) });
      } catch (e) {
        report({ error: String((e && e.message) || e) });
      }
    },
    // Decodes a voice note to 48 kHz mono 16-bit PCM (what Mesh's speaker
    // output plays) and hands it back through waPcm.
    voicePcm: async (id) => {
      const report = (r) => window.waPcm(JSON.stringify({ id, ...r }));
      try {
        const encoded = await (await decrypt(id)).arrayBuffer();
        // decodeAudioData resamples to the context's rate.
        const audio = await new OfflineAudioContext(1, 1, 48000).decodeAudioData(encoded);
        const channels = [...Array(audio.numberOfChannels)].map((_, c) => audio.getChannelData(c));
        const pcm = new Int16Array(audio.length);
        for (let i = 0; i < pcm.length; i++) {
          let sum = 0;
          for (const ch of channels) sum += ch[i];
          pcm[i] = Math.max(-1, Math.min(1, sum / channels.length)) * 32767;
        }
        report({ data: await b64(new Blob([pcm.buffer])) });
      } catch (e) {
        report({ error: String((e && e.message) || e) });
      }
    },
    // "wa-msg:<id>" is a message's picture for the chat, "wa-big:<id>" a
    // bigger one for full screen; anything else is a URL (profile pictures)
    // fetched through the page. Mesh gets base64 through waMedia. "wa-gif:<id>"
    // answers through waFrames instead, with the GIF's frames.
    fetchMedia: async (url) => {
      const report = (r) => window.waMedia(JSON.stringify({ url, ...r }));
      try {
        let blob;
        if (url.startsWith("wa-gif:")) {
          try {
            window.waFrames(JSON.stringify({ url, delay_ms: Math.round(1000 / FPS), frames: await gifFrames(url.slice(7)) }));
          } catch (e) {
            window.waFrames(JSON.stringify({ url, error: String((e && e.message) || e) }));
          }
          return;
        }
        if (url.startsWith("wa-msg:") || url.startsWith("wa-big:")) {
          const m = wa.Msg.get(url.slice(7));
          blob = await shrink(await decrypt(url.slice(7)), !!m && m.type === "sticker", url.startsWith("wa-big:") ? 1600 : 480);
        }
        else {
          const res = await fetch(url);
          if (!res.ok) return report({ error: `HTTP ${res.status}` });
          blob = await res.blob();
        }
        report({ data: await b64(blob) });
      } catch (e) {
        // Old media expires from WhatsApp's servers (it then lives only on the phone).
        report({ error: String((e && e.message) || e) });
      }
    },
  };

  // Stores fill in after login; keep looking until they're there.
  const boot = () => {
    wa = find();
    if (!wa) return setTimeout(boot, 1000);
    wa.Msg.on("add", onMessage);
    wa.Msg.on("change", (m) => { if (m && m.id && String(m.id.remote) === open) emitMessages(); });
    for (const e of ["add", "remove", "change:t", "change:unreadCount", "change:archive"]) wa.Chat.on(e, emit);
    if (wa.Pics) for (const e of ["add", "change"]) wa.Pics.on(e, emit);
    // A reaction lands on its parent message (same id) in the open chat.
    if (wa.Reactions) for (const e of ["add", "change", "remove"]) wa.Reactions.on(e, (r) => {
      const parent = r && wa.Msg.get(r.id);
      if (parent && String(parent.id.remote) === open) emitMessages();
    });
    emit();
  };
  boot();
  return "installed";
})();
