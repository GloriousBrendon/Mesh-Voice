// Runs inside WhatsApp Web (a tab of the engine browser). Reads WhatsApp's
// own collections (Haste modules, required by name) and calls its own
// actions, so Mesh can show and send WhatsApp chats without the window.
// Reports through the waEmit / waMessages / waNotify / waMedia bindings.
// See design/WHATSAPP-NATIVE.md for the modules used.
(() => {
  // Bump when the API below changes, so a running page takes the new agent.
  const version = 3;
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
  const snapshot = () => {
    const chats = wa.Chat.getModelsArray()
      .filter((c) => !c.archive)
      .sort((a, b) => (b.t || 0) - (a.t || 0))
      .slice(0, CHATS);
    askPictures(chats.slice(0, 40));
    return {
      me: String(me()),
      chats: chats.map((c) => ({
        id: String(c.id),
        name: title(c),
        group: isGroup(c.id),
        unread: Math.max(0, c.unreadCount || 0),
        last_ms: (c.t || 0) * 1000,
        muted: muted(c),
        avatar: pic(c.id),
      })),
    };
  };

  let pending = null;
  const emit = () => {
    if (pending || !wa) return;
    pending = setTimeout(() => {
      pending = null;
      try { window.waEmit(JSON.stringify(snapshot())); } catch (e) { console.warn("mesh wa emit", e); }
    }, 150);
  };

  // Media kinds Mesh shows as a label (with any caption) rather than inline.
  const LABEL = { video: "Video", ptt: "Voice message", audio: "Audio", document: "Document", location: "Location", vcard: "Contact" };
  const SYSTEM = { revoked: "deleted a message", call_log: "called", ciphertext: "sent a message WhatsApp is still decrypting" };
  const message = (m) => {
    const fromMe = !!m.id.fromMe;
    const sender = m.senderObj;
    const authorId = String(fromMe ? me() : m.author || m.from);
    const author = fromMe ? "You" : (sender && (sender.name || sender.pushname || sender.verifiedName)) || authorId.split("@")[0];
    let content, system = false, image = null;
    if (m.type === "chat") content = m.body || "";
    else if (m.type === "image") {
      content = m.caption || "";
      image = { width: m.width || null, height: m.height || null, sticker: false };
    } else if (m.type === "sticker") {
      content = "";
      image = { width: m.width || 512, height: m.height || 512, sticker: true };
    } else if (LABEL[m.type]) content = `[${LABEL[m.type]}${m.type === "document" && m.filename ? `: ${m.filename}` : ""}]${m.caption ? ` ${m.caption}` : ""}`;
    else if (SYSTEM[m.type]) { content = SYSTEM[m.type]; system = true; }
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

  const onMessage = (m) => {
    const chatId = String(m.id.remote);
    if (chatId === open) emitMessages();
    emit();
    // A desktop notification for new messages from others in unmuted chats.
    if (!m.isNewMsg || m.id.fromMe) return;
    const chat = wa.Chat.get(m.id.remote);
    const shown = message(m);
    if (!chat || muted(chat) || !shown || shown.system) return;
    const name = isGroup(chatId) ? `${shown.author} in ${title(chat)}` : shown.author;
    const body = shown.content || (shown.image ? "Photo" : "");
    try { window.waNotify(JSON.stringify({ chat_id: chatId, title: name, body })); } catch {}
  };

  // Shrinks a picture so Mesh gets a few KB: photos to 480 px wide as JPEG,
  // stickers to 160 px as PNG (keeping their transparency). ponytail: an
  // animated sticker shows its first frame.
  const shrink = async (blob, sticker) => {
    const bitmap = await createImageBitmap(blob);
    const scale = Math.min(1, (sticker ? 160 : 480) / bitmap.width);
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

  window.__mesh = {
    version,
    ready: () => !!wa,
    snapshot: () => (wa ? JSON.stringify(snapshot()) : null),
    open: (chatId) => {
      open = chatId || null;
      emitMessages();
    },
    send: (chatId, text) => {
      const chat = wa.Chat.get(chatId);
      if (chat) return wa.send.sendTextMsgToChat(chat, text);
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
    // "wa-msg:<message id>" decrypts that message's image; anything else is a
    // URL (profile pictures) fetched through the page. Either way Mesh gets
    // base64 through waMedia.
    fetchMedia: async (url) => {
      const report = (r) => window.waMedia(JSON.stringify({ url, ...r }));
      try {
        let blob;
        if (url.startsWith("wa-msg:")) {
          const m = wa.Msg.get(url.slice(7));
          blob = await shrink(await decrypt(url.slice(7)), !!m && m.type === "sticker");
        }
        else {
          const res = await fetch(url);
          if (!res.ok) return report({ error: `HTTP ${res.status}` });
          blob = await res.blob();
        }
        const bytes = new Uint8Array(await blob.arrayBuffer());
        let binary = "";
        for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
        report({ data: btoa(binary) });
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
    emit();
  };
  boot();
  return "installed";
})();
